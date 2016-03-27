
use std::num::ParseIntError;
use std::fmt::{self, Display};
use std::convert::From;
use std::sync::Arc;
use std::error::{Error};
use std;

use ::regex::{self, Regex};
use ::redis::{self, RedisResult, Commands};
use ::shiplift::{self, Docker};
use ::chan_signal::{self, Signal};
use ::chan;
use ::rustc_serialize::json::{self, ToJson};

use super::{Config, MissingEnvVarHandling, MissingContainerHandling};

/// Specifiction for a single domain.
/// Contains either an http, an https or both ports.
#[derive(Debug)]
struct DomainSpec {
    domain_name: String,
    http_port: Option<u16>,
    https_port: Option<u16>,
}

const DS_PAT: &'static str = r"([a-zA-Z0-9][a-zA-Z0-9.-]+[a-zA-Z0-9]\.?)(:(\S+))?";

impl DomainSpec {
    pub fn spec_id(&self) -> String {
        return self.domain_name.replace(".","_")
    }
}

fn optional_result<R,E>(x_opt: Option<Result<R,E>>) -> Result<Option<R>, E> {
    match x_opt {
        Some(Ok(x)) => Ok(Some(x)),
        Some(Err(e)) => Err(e),
        None => Ok(None)
    }
}

struct Context {
    redis_client: Option<redis::Client>,
    docker_client: Option<Docker>,
    config: Arc<Config>,
    termination_signal: chan::Receiver<Signal>,
    domain_spec_pat: Regex
}

impl Context {
    fn new(config: Arc<Config>, termination_signal:  chan::Receiver<Signal>) -> Result<Context, CompanionError> {
        let domain_spec_pat = try!(Regex::new(DS_PAT));
        Ok(Context { redis_client: None, docker_client: None,
            config: config,
            termination_signal: termination_signal,
            domain_spec_pat: domain_spec_pat
        })
    }

    fn create_redis_client(&mut self) -> RedisResult<&mut redis::Client> {
        if let Some(ref mut client) = self.redis_client {
            Ok(client)
        } else {
            let addr = redis::ConnectionAddr::Tcp(
                self.config.redis_host.clone(),
                self.config.redis_port);
            let info = redis::ConnectionInfo {
                addr: Box::new(addr),
                db: 0,
                passwd: None
            };
            let client = try!(redis::Client::open(info));
            self.redis_client = Some(client);
            Ok(self.redis_client.as_mut().unwrap())
        }
        // Currently, we create a new connection for each container we refresh.
        // TODO: consider caching/maintaining a redis connection for batch updates
        // If only a single container is being updated, it's fine to re-establish the connection
        // once per timeout.
    }

    fn create_docker_client(&mut self) -> &mut Docker {
        if let Some(ref mut client) = self.docker_client {
            client
        } else {
            let client = Docker::host(self.config.docker_url.clone());
            self.docker_client = Some(client);
            self.docker_client.as_mut().unwrap()
        }
    }

    fn update_registration_for_container_internal(&mut self, container_name: &str)
            -> Result<(),CompanionError> {
        info!("Refreshing beachhead config for {}", container_name);

        // inspect docker container
        let config = self.config.clone();
        let (container_host, env_opt) = {
            let docker = self.create_docker_client();
            let containers = docker.containers();
            let container_ref = containers.get(container_name);
            let container = try!(container_ref.inspect());
            // TODO: check here if shutdown is requested (SIGTERM, SIGINT)
            // There could be a long wait on `inspect()`. We could have been interrupted in the meantime.
            // Shouldn't publish new config to redis *after* termination has been requested, if possible.

            // When docker network is active, we use the container name (=hostname)
            // otherwise, we use "IPAddress", which hopefully maps to the ip on the bridge interface.
            // At this point, the shiplift library doesn't know about 'docker networks'
            let container_host = if config.docker_network {
                container_name.to_owned()
            } else {
                container.NetworkSettings.IPAddress
            };


            let env_opt = container.Config.Env;
            (container_host, env_opt)
        };
        let mut specs = vec!();
        if let Some(env) = env_opt {
            for line in env.iter() {
                let parts : Vec<&str> = line.splitn(2,'=').collect();
                if parts.len() < 2 || &parts[0] != &config.envvar {
                    continue;
                }
                try!(self.parse_all_domain_specs(&parts[1], &mut specs));
            }
        }
        else {
            // TODO implement missingenvvarhandling::automatic
            match config.missing_envvar {
                MissingEnvVarHandling::Ignore => (),
                MissingEnvVarHandling::Automatic | MissingEnvVarHandling::Report => {
                    error!("No environment variable {} on container {}.", config.envvar, container_name);
                }
            }
        }


        let mut published_config = json::Array::new();
        for spec in specs {
            fn svc_config<T: ToJson>(domain_config: &mut json::Object, field: &str, value_opt: Option<T>) {
                if let Some(value) = value_opt {
                    domain_config.insert(field.to_owned(), value.to_json());
                }
            }
            fn backend_setup(host: &str, port: u16) -> Option<json::Object> {
                let mut setup = json::Object::new();
                setup.insert("host".to_owned(), host.to_owned().to_json());
                setup.insert("port".to_owned(), port.to_owned().to_json());
                Some(setup)
            }
            let mut domain_config = json::Object::new();
            svc_config(&mut domain_config, "id", Some(spec.spec_id()));
            svc_config(&mut domain_config, "domain", Some(spec.domain_name));
            svc_config(&mut domain_config, "http", spec.http_port.map(|http_port|
                                                                      backend_setup(&container_host, http_port)));
            svc_config(&mut domain_config, "https", spec.https_port.map(|https_port|
                                                                        backend_setup(&container_host, https_port)));
            published_config.push(domain_config.to_json());
        }
        let mut key = String::new();
        self.service_key(container_name, &mut key);


        let redis_value = try!(json::encode(&published_config));

        // Set key in redis
        let r_client = try!(self.create_redis_client());


        if let Some(expire_seconds) = config.expire_seconds {
            try!(r_client.set_ex(key, redis_value, expire_seconds as usize));
        } else {
            try!(r_client.set(key, published_config));
        }
        Ok(())
    }
    
    fn update_registration_for_container(&mut self, container_name: &str)
            -> Result<(),ContainerRefreshError<CompanionError>> {

        try_!(self.update_registration_for_container_internal(container_name), container_name.to_owned());
        Ok(())
    }

    fn wait(&mut self) -> bool {
        if let Some(refresh_seconds) = self.config.refresh_seconds {
            let rsig = &mut self.termination_signal;
            let timeout_duration = std::time::Duration::from_secs(refresh_seconds as u64);
            let refresh_timeout = chan::after(timeout_duration);
            let do_continue : bool;
            chan_select! {
                rsig.recv() => {
                    do_continue = false
                },
                refresh_timeout.recv() => {
                    // just continue with the loop
                    do_continue = true
                },
            };
            do_continue
        } else {
            // Only refresh once and then exit.
            false
        }
    }

    fn service_key(&self, container_name: &str, key: &mut String) {
        key.push_str(&self.config.key_prefix);
        key.push_str(container_name);
    }

    pub fn parse_all_domain_specs(&self, raw: &str, specs: &mut Vec<DomainSpec>) -> Result<(),DomainSpecError> {
        fn parse_port(key: &str, value: Option<&str>, spec: &DomainSpec) -> Result<Option<u16>, DomainSpecError> {
            match optional_result(value.map(|v| u16::from_str_radix(v,10))) {
                Err(e) => Err(DomainSpecError { domain_name: spec.domain_name.clone(), cause: e, key: Some(key.to_owned()) }),
                Ok(port) => Ok(port)
            }
        }
        for captures in self.domain_spec_pat.captures_iter(raw) {
            // The first capture group is guaranteed to be there.
            let mut domain_name = captures.at(1).unwrap();

            // Strip . at the end of FQDN
            if domain_name.ends_with('.') {
                // Chop off '.' at the end
                domain_name = &domain_name[0..(domain_name.len() - 1)]
            }

            let raw_params = captures.at(3)
            .map(|params| params.trim().split(':').collect())
            .unwrap_or_else(||Vec::new());

            let mut spec = DomainSpec {
                domain_name: domain_name.to_owned(),
                http_port: None,
                https_port: None
            };
            for raw_param in raw_params {
                let param_parts : Vec<&str> = raw_param.splitn(2,'=').collect();

                let key = param_parts[0].trim().to_lowercase();
                let value = if param_parts.len() > 1 {
                    Some(param_parts[1].trim())
                } else {
                    None
                };

                // Merely having an 'http' or 'https' key present, enables the mapping
                match key.as_str() {
                    "http" => {
                        let parsed_port_opt = try!(parse_port(&key, value, &spec));
                        spec.http_port = Some(parsed_port_opt.unwrap_or(80));
                    },
                    "https" => {
                        let parsed_port_opt = try!(parse_port(&key, value, &spec));
                        spec.https_port = Some(parsed_port_opt.unwrap_or(443));
                    },
                    _ => {
                        warn!("Unknown domain spec parameter ");
                    }
                }
            }

            // If neither http nor https have been specified, assume both mappings.
            if spec.http_port.is_none() && spec.https_port.is_none() {
                spec.http_port = Some(80);
                spec.https_port = Some(443);
            }

            specs.push(spec);
        }

        Ok(())
    }
}

pub fn main(config: Arc<Config>, container_names: Vec<String>) -> Result<(), CompanionAggregateError> {
    // TODO: implement --enumerate
    let abort_signal = chan_signal::notify(&[Signal::INT, Signal::TERM]);
    let mut ctx = try!(Context::new(config.clone(), abort_signal));
    loop {
        for container_name in &container_names {
            try!(ctx.update_registration_for_container(&container_name));
        }
        if !ctx.wait() {
            break;
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct DomainSpecError {
    pub domain_name: String,

    // move into enum when more options come
    pub cause: ParseIntError,
    pub key: Option<String>
}

impl Display for DomainSpecError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        try!(write!(fmt, "{} Domain name: \"{}\"", self.description(), self.domain_name));
        if let Some(ref key) = self.key {
            try!(write!(fmt, " Option name: \"{}\"", key));
        }
        write!(fmt, " Cause: {}", self.cause)
    }
}

impl Error for DomainSpecError {
    fn description(&self) -> &str {
        "Failed to parse domain spec option."
    }
    fn cause(&self) -> Option<&Error> {
        Some(&self.cause)
    }
}

#[derive(Debug)]
pub struct ContainerRefreshError<T: Error> {
    pub container_name: String,
    pub cause: T
}

impl<T: Error> Error for ContainerRefreshError<T> {
    fn description(&self) -> &str {
        "Failed to refresh container configuration."
    }
    fn cause(&self) -> Option<&Error> { Some(&self.cause) }
}

impl<T: Error+Display> Display for ContainerRefreshError<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        try!(write!(fmt, "{} Container name: {}. Cause: ", self.description(), self.container_name));
        Display::fmt(&self.cause, fmt)
    }
}

impl<T: Error> From<(T, String)> for ContainerRefreshError<T> {
    fn from(value: (T, String)) -> ContainerRefreshError<T> {
        let (e, container_name) = value;
        ContainerRefreshError { container_name: container_name, cause: e}
    }
}

// TODO: would be cool if shiplift::errors::Error supported std::error::Error

quick_error! {
    #[derive(Debug)]
    pub enum CompanionError {
        DomainSpecParsing(err: DomainSpecError) {
            cause(err)
            from()
            display("{}", err)
        }
        Redis(err: redis::RedisError) {
            cause(err)
            from()
            display("{}", err)
        }
        Docker(err: shiplift::errors::Error) {
            from()
            display("{:?}", err)
        }
        ParseInt(err: std::num::ParseIntError) {
            cause(err)
            from()
            display("{}", err)
        }
        JsonEncoder(err: json::EncoderError) {
            cause(err)
            from()
            display("{}", err)
        }
        Regex(err: regex::Error) {
            cause(err)
            from()
            display("{}", err)
        }
    }
}

quick_error! {
    #[derive(Debug)]
    pub enum CompanionAggregateError {
        Top(err: CompanionError) {
            cause(err)
            from()
            display("{}", err)
        }
        ContainerRefresh(err: ContainerRefreshError<CompanionError>){
            cause(err)
            from()
            display("{}", err)
        }
    }
}
