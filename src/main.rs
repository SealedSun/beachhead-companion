
use std::env;
use std::io::{stderr, Write};
use std::fmt::Display;
use std::sync::Arc;

extern crate docopt;
extern crate regex;
extern crate redis;
extern crate shiplift;
extern crate env_logger;
extern crate chrono;
extern crate rustc_serialize;
extern crate url;
#[macro_use]
extern crate chan;
extern crate chan_signal;

#[macro_use]
extern crate quick_error;

#[macro_use]
extern crate log;

use ::url::Url;

#[macro_use]
mod common;
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
                        0 means set once and then exit. [default: None]
    --key-prefix=KEY    Key prefix to use in redis. Will be followed by container name. [default: /beachhead/]
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
                        for containers that are listed explicityly unless --ignore-missing-envvar
                        is present.
    --ignore-missing-envvar
                        Ignore missing `envvar` environment variables. Automatically enabled on
                        containers that are not explicityly listed unless --error-missing-envvar
                        is present.
    --error-missing-container
                        Consider an explicityly listed container that is missing/not running an
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

const VERSION: &'static str = env!("CARGO_PKG_VERSION");

/// Holds arguments parsed by docopt. Will be transferred into [Config].
#[derive(RustcDecodable)]
struct Args {
    flag_verbose: bool,
    flag_quiet: bool,
    arg_redis_host: String,
    arg_redis_port: u16,
    arg_expire: u32,
    arg_refresh: Option<u32>,
    arg_docker_url: Url,
    arg_envvar: String,
    arg_containers: Vec<String>,
    arg_key_prefix: String,
    flag_docker_network: bool,
    flag_dry_run: bool,
    flag_error_missing_envvar: bool,
    flag_error_missing_container: bool,
    flag_ignore_missing_envvar: bool
}

fn main() {
    // Parse arguments (handles --help and --version)
    let mut args: Args = docopt::Docopt::new(USAGE)
                             .and_then(|d| {
                                 d.help(true)
                                  .version(Some(String::from(VERSION)))
                                  .decode()
                             })
                             .unwrap_or_else(|e| e.exit());

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
    if args.arg_refresh.is_none() {
        args.arg_refresh = Some(((args.arg_expire as f64) * 0.45) as u32);
    }

    stay_calm_and(init_log(&args));
    let mut containers = Vec::with_capacity(args.arg_containers.len());
    containers.append(&mut args.arg_containers);
    let config = Arc::new(Config::from_args(args));
    stay_calm_and(companion::main(config, containers));
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

/// Display errors a bit more nicely, depending on whether logging is enabled or not.
fn stay_calm_and<T, E>(result: Result<T, E>)
    where E: Display
{
    match result {
        Ok(_) => (),
        Err(e) => {
            // We need erros to be shown to the user. If we can, we use the error logging mechanism.
            // Otherwise, we just print to stderr.
            if log_enabled!(log::LogLevel::Error) {
                error!("Fatal error: {}", e);
            } else {
                match writeln!(&mut stderr(), "Fatal error: {}", e) {
                    Err(_) => (), // ignore, nothing left to do
                    Ok(_) => (),
                }
            }
            std::process::exit(100);
        }
    }
}

/// The configuration used by beachhead-companion. Matches [Args] mostly.
/// Note that the meanings of Option and 0 change to match program logic
/// more naturally.
pub struct Config {
    /// The hostname or ip address of the redis server.
    pub redis_host: String,
    /// The port of the redis server.
    pub redis_port: u16,
    /// The prefix for the keys to insert into redis. Will be followed by the container name.
    pub key_prefix: String,
    /// The expiration for registrations in seconds. None means no expiration.
    /// If there is some expiration
    pub expire_seconds: Option<u32>,
    /// The refresh interval for registrations in seconds. None means no refresh,
    /// only set once.
    pub refresh_seconds: Option<u32>,
    /// URL to the docker socket.
    pub docker_url: Url,
    /// Whether to use the container hostname (true) or lookup the bridge network IP.
    pub docker_network: bool,
    /// Name of the environment variable to look up in docker container configuration.
    pub envvar: String,
    /// Indicates whether this is a dry-run where the Redis update is not performed.
    pub dry_run: bool,
    /// How to handle missing environment variables on containers.
    pub missing_envvar: MissingEnvVarHandling,
    /// How to handle missing containers.
    pub missing_container: MissingContainerHandling,
}

impl Config {
    fn from_args(args: Args) -> Config {
        Config {
            redis_host: args.arg_redis_host,
            redis_port: args.arg_redis_port,
            key_prefix: args.arg_key_prefix,
            docker_url: args.arg_docker_url,
            envvar: args.arg_envvar,
            dry_run: args.flag_dry_run,
            expire_seconds: if args.arg_expire == 0 {
                None
            } else {
                Some(args.arg_expire)
            },
            refresh_seconds: args.arg_refresh
                                 .map(|r| {
                                     if r == 0 {
                                         None
                                     } else {
                                         Some(r)
                                     }
                                 })
                                 .unwrap(),
            docker_network: args.flag_docker_network,
            missing_envvar: match (args.flag_error_missing_envvar, args.flag_ignore_missing_envvar) {
                (true, true) => MissingEnvVarHandling::Automatic,
                (false, false) => MissingEnvVarHandling::Automatic,
                (true, _) => MissingEnvVarHandling::Report,
                (_, true) => MissingEnvVarHandling::Ignore
            },
            missing_container: if args.flag_error_missing_container {
                MissingContainerHandling::Report
            } else {
                MissingContainerHandling::Ignore
            },
        }
    }
}

pub enum MissingEnvVarHandling {
    Automatic,
    Report,
    Ignore
}

pub enum MissingContainerHandling {
    Report,
    Ignore
}

#[cfg(test)]
mod test {
    use docopt;

    #[test]
    fn docopt_spec() {
        docopt::Docopt::new(super::USAGE).unwrap();
    }
}
