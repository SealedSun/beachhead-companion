
use std::env;
use std::sync::Arc;
use std::rc::Rc;

use common::{stay_calm_and, stay_very_calm_and, Config, MissingContainerHandling, MissingEnvVarHandling};

extern crate docopt;
extern crate regex;
extern crate redis;
extern crate shiplift;
extern crate env_logger;
extern crate chrono;
extern crate rustc_serialize;
extern crate url;
extern crate chan_signal;

#[macro_use]
extern crate log;
#[macro_use]
extern crate chan;
#[macro_use]
extern crate quick_error;
#[macro_use]
extern crate lazy_static;

use url::Url;
use docopt::Docopt;
use chan_signal::Signal;

#[macro_use]
mod common;
mod domain_spec;
mod inspector;
mod publisher;
mod companion;

#[cfg_attr(rustfmt, rustfmt_skip)]
const USAGE: &'static str = "
Usage: beachhead-companion [options] [--ignore-missing-envvar] [--error-missing-container] [--] <containers>...
       beachhead-companion [options] [--error-missing-envvar] --enumerate
       beachhead-companion --help
       beachhead-companion --version

Options:
    -h, --help          Show help (this message).
    --version           Show the version of beachhead-companion.
    --verbose           Show additional diagnostic output.
    --quiet             Only show warnings and errors.
    --redis-host=HOST   Hostname or IP of the Redis server [default: localhost]
    --redis-port=PORT   Port of the Redis server [default: 6379]
    --expire=SECONDS    Number of seconds after which to expire registration.
                        0 means no expiration. [default: 60]
    --refresh=SECONDS   Number of seconds after which to refresh registrations.
                        Defaults to 45% of the expiration time. At least 10 seconds.
                        0 means set once and then exit.
    --key-prefix=KEY    Key prefix to use in redis. Will be followed by container name.
                        [default: /beachhead/]
    --docker-url=URL    URL to the docker socket. [default: unix://var/run/docker.sock]
    --docker-network    Whether to use the container hostname (set) or use the bridge
                        network IP (unset/default).
    --envvar=VAR        Name of the environment variable to look for in the container.
                        [default: BEACHHEAD_DOMAINS]
    --enumerate         Ask docker daemon for list of all running containers instead of
                        passing individual container names/ids. Enumeration will be repeated
                        on each refresh (containers can come and go)
    --error-missing-envvar
                        Consider `envvar` missing on a container an error. Automatically enabled
                        for containers that are listed explicitly unless --ignore-missing-envvar
                        is present.
    --ignore-missing-envvar
                        Ignore missing `envvar` environment variables. Automatically enabled on
                        containers that are not explicitly listed unless --error-missing-envvar
                        is present.
    --error-missing-container
                        Consider an explicitly listed container that is missing/not running an
                        error. Defaults to false as it isn't really beachhead-companion's job
                        to monitor your containers.
    -n, --dry-run       Don't update registrations, just check container status and configuration.
                        Ignores --quiet.

The docker container with the supplied name needs to exist and have the BEACHHEAD_DOMAINS
environment variable set (or whatever is configured).
The environment variable lists 'domain-specs' separated by spaces. A domain-spec has the format
'DOMAIN[:http[=PORT]][:https[=PORT]]'. If neither 'http' not 'https' is specified, both
are assumed. Default ports are 80 for HTTP and 443 for HTTPS. Whether HTTP/2.0 is supported
or not does not concern the beachhead. If both the 'naked' and a 'www.' domain need to be
supported, you need to add both domains to the list.

Example:
  BEACHHEAD_DOMAINS=example.org admin.example.org:https app.example.org:http=8080:https=8043
    is parsed as
  example.org with http=80, https=443
  admin.example.org with https=443
  app.example.org with http=8080 and https=8043

One way to use beachhead-companion is to supply an explicit list of container names/ids to check
for domain specifications. Alternatively, you can have beachhead-companion check all containers
via the `--enumerate` flag.

Supports more fine-grained logging control via the RUST_LOG environment variable.
See http://rust-lang-nursery.github.io/log/env_logger for details.
";

lazy_static! {
    static ref DOCOPT: Docopt = Docopt::new(USAGE).expect("docopt failed to parse USAGE")
        .help(true).version(Some(String::from(VERSION)));
}

const VERSION: &'static str = env!("CARGO_PKG_VERSION");

/// Holds arguments parsed by [docopt]. Will be transferred into [common/Config].
#[derive(RustcDecodable, Clone)]
struct Args {
    flag_verbose: bool,
    flag_quiet: bool,
    flag_redis_host: String,
    flag_redis_port: u16,
    flag_expire: u32,
    flag_refresh: Option<u32>,
    flag_docker_url: Url,
    flag_envvar: String,
    flag_key_prefix: String,
    arg_containers: Vec<String>,
    flag_docker_network: bool,
    flag_dry_run: bool,
    flag_error_missing_envvar: bool,
    flag_error_missing_container: bool,
    flag_ignore_missing_envvar: bool,
    flag_enumerate: bool,
}

// Implement Default by parsing an (almost) empty command line.
// That way, the defaults are only stated once (in the USAGE)
impl Default for Args {
    fn default() -> Args {
        // We use the enumerate form of the command and then strip the enumerate flag away again.
        let argv = vec!["beachhead-companion", "--enumerate"];
        let mut args: Args = DOCOPT.clone().argv(argv).decode().unwrap();
        args.flag_enumerate = false;
        args
    }
}

impl Args {
    fn deconstruct(self) -> (Config,Vec<String>) {
        let config = Config {
            redis_host: Rc::new(self.flag_redis_host),
            redis_port: self.flag_redis_port,
            key_prefix: Rc::new(self.flag_key_prefix),
            docker_url: self.flag_docker_url,
            enumerate: self.flag_enumerate,
            envvar: Rc::new(self.flag_envvar),
            dry_run: self.flag_dry_run,
            expire_seconds: if self.flag_expire == 0 {
                None
            } else {
                Some(self.flag_expire)
            },
            refresh_seconds: self.flag_refresh
                                 .map(|r| {
                                     if r == 0 {
                                         None
                                     } else {
                                         Some(r)
                                     }
                                 })
                                 .unwrap(),
            docker_network: self.flag_docker_network,
            missing_envvar: match (self.flag_error_missing_envvar,
                                   self.flag_ignore_missing_envvar) {
                (true, true) => MissingEnvVarHandling::Automatic,
                (false, false) => MissingEnvVarHandling::Automatic,
                (true, _) => MissingEnvVarHandling::Report,
                (_, true) => MissingEnvVarHandling::Ignore,
            },
            missing_container: if self.flag_error_missing_container {
                MissingContainerHandling::Report
            } else {
                MissingContainerHandling::Ignore
            }
        };
        (config, self.arg_containers)
    }
}

fn args_transform(args: &mut Args) {
    // Apply some args transformation rules

    // quiet and verbose cancel each other out
    if args.flag_quiet && args.flag_verbose {
        args.flag_quiet = false;
        args.flag_verbose = false;
    }

    // dry-run implies !quiet
    if args.flag_dry_run {
        args.flag_quiet = false;
    }

    // refresh := refresh || 45% of expire
    // Note: expire has a default set by docopt (not replicated in the Default impl)
    if args.flag_refresh.is_none() {
        args.flag_refresh = Some(((args.flag_expire as f64) * 0.45) as u32);
    }
}

fn main() {
    // Parse arguments (handles --help and --version)
    let mut args: Args = DOCOPT.decode()
                               .unwrap_or_else(|e| e.exit());

    args_transform(&mut args);

    stay_calm_and(init_log(&args));
    let (config, arg_containers) = args.deconstruct();
    let config = Arc::new(config);
    let mut containers = Vec::with_capacity(arg_containers.len());
    containers.extend(arg_containers.into_iter().map(|x| Rc::new(x)));
    let abort_signal = chan_signal::notify(&[Signal::INT, Signal::TERM]);
    let docker_inspector = Box::new(inspector::docker::DockerInspector::new(config.clone()));
    let redis_publisher = Box::new(publisher::redis::RedisPublisher::new(config.clone()));

    stay_very_calm_and(companion::run(
        config, docker_inspector, redis_publisher, abort_signal, &containers));
}

/// Handles the verbosity options by initializing the logger accordingly.
/// Can be overridden using RUST_LOG.
fn init_log(args: &Args) -> Result<(), log::SetLoggerError> {
    // initialize logging (depending on flags)
    let mut log_builder = env_logger::LogBuilder::new();
    log_builder.format(|record| {
        format!("{} [{}] {}: {}",
                chrono::Local::now(),
                record.location().module_path(),
                record.level(),
                record.args())
    });
    let level = match (args.flag_verbose, args.flag_quiet) {
        (false, false) => log::LogLevelFilter::Info,
        (true, _) => log::LogLevelFilter::Debug,
        (_, true) => log::LogLevelFilter::Warn,
    };
    log_builder.filter(Some("beachhead-companion"), level);
    if let Ok(rust_log) = env::var("RUST_LOG") {
        log_builder.parse(&rust_log);
    }
    log_builder.init()
}

#[cfg(test)]
mod test {
    use super::{USAGE, args_transform, Args};
    use docopt;
    use common;

    #[test]
    fn docopt_spec() {
        docopt::Docopt::new(USAGE).unwrap();
    }

    #[test]
    fn args_quiet_verbose() {
        common::init_log();
        // #### GIVEN ####
        let mut args: Args = Default::default();
        args.flag_quiet = true;
        args.flag_verbose = true;

        // #### WHEN  ####
        args_transform(&mut args);

        // #### THEN  ####
        assert_eq!(args.flag_quiet, false);
        assert_eq!(args.flag_verbose, false);
    }

    #[test]
    fn args_quiet() {
        common::init_log();
        // #### GIVEN ####
        let mut args: Args = Default::default();
        args.flag_quiet = true;
        args.flag_verbose = false;

        // #### WHEN  ####
        args_transform(&mut args);

        // #### THEN  ####
        assert_eq!(args.flag_quiet, true);
        assert_eq!(args.flag_verbose, false);
    }

    #[test]
    fn args_verbose() {
        common::init_log();
        // #### GIVEN ####
        let mut args: Args = Default::default();
        args.flag_quiet = false;
        args.flag_verbose = true;

        // #### WHEN  ####
        args_transform(&mut args);

        // #### THEN  ####
        assert_eq!(args.flag_quiet, false);
        assert_eq!(args.flag_verbose, true);
    }

    #[test]
    fn args_refresh_default() {
        common::init_log();
        // #### GIVEN ####
        let mut args: Args = Default::default();
        args.flag_expire = 60;
        args.flag_refresh = None;

        // #### WHEN  ####
        args_transform(&mut args);

        // #### THEN  ####
        assert!(args.flag_refresh.is_some(),
                "flag_refresh should have a default");
        assert!(args.flag_refresh.unwrap() < args.flag_expire,
                "flag_refresh {} must be smaller than expire (60)",
                args.flag_refresh.unwrap());
        assert!(args.flag_refresh.unwrap() <= args.flag_expire / 2,
                "flag_refresh {} must be no more than half of expire (30)",
                args.flag_refresh.unwrap());
        assert!(args.flag_refresh.unwrap() >= args.flag_expire / 3,
                "flag_refresh {} must be at least a third of expire (20)",
                args.flag_refresh.unwrap());
    }

    #[test]
    fn args_refresh_custom() {
        common::init_log();
        // #### GIVEN ####
        let mut args: Args = Default::default();
        args.flag_expire = 60;
        args.flag_refresh = Some(50);

        // #### WHEN  ####
        args_transform(&mut args);

        // #### THEN  ####
        assert_eq!(args.flag_expire, 60);
        assert_eq!(args.flag_refresh, Some(50));
    }

    #[test]
    fn default_args_valid_config() {
        common::init_log();
        // #### GIVEN ####
        let mut args: Args = Default::default();
        args_transform(&mut args);
        let args_expire = args.flag_expire;

        // #### WHEN  ####
        let config = args.into_config();

        // #### THEN  ####
        assert_eq!(config.expire_seconds, Some(args_expire));
    }

}
