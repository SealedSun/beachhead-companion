
use std::env;
use std::io::{stderr, Write};
use std::fmt::Display;

extern crate docopt;
extern crate regex;
extern crate redis;
extern crate shiplift;
extern crate env_logger;
extern crate chrono;
extern crate rustc_serialize;

#[macro_use]
extern crate log;

#[cfg_attr(rustfmt, rustfmt_skip)]
const USAGE: &'static str = "
Usage: beachhead-companion [options] [--] [<container>...]
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
    --docker-url        URL to the docker socket. [default: unix://var/run/docker.sock]
    --envvar=VAR        Name of the environment variable to look for in the container.
                        [default: BEACHHEAD_DOMAINS]
    -n, --dry-run       Don't update registrations, just check container status and configuration.
                        Ignores --quiet.

The docker container with the supplied name needs to exist and have the BEACHHEAD_DOMAINS
environment variable set (or whatever is configured).
It lists 'domain-specs' separated by spaces. A domain-spec has the format
'DOMAIN[:http[=PORT]][:https[=PORT]]'. If neither 'http' not 'https' is specified, both
are assumed. Default ports are 80 for HTTP and 443 for HTTPS. Whether HTTP/2.0 is supported
or not does not concern the beachhead. If both the 'naked' and a 'www.' domain need to be
supported, you need to add both domains to the list.
";

const VERSION: &'static str = env!("CARGO_PKG_VERSION");

#[derive(RustcDecodable)]
struct Args {
    flag_verbose: bool,
    flag_quiet: bool,
    arg_redis_host: String,
    arg_redis_port: u16,
    arg_expire: u32,
    arg_refresh: Option<u32>,
    arg_docker_url: String,
    arg_envvar: String,
    flag_dry_run: bool,
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
}

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
    if env::var("RUST_LOG").is_ok() {
        log_builder.parse(&env::var("RUST_LOG").unwrap());
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

struct Config {
    redis_host: String,
    redis_port: u16,
    expire: i32,
    refresh: i32,
    docker_url: String,
    envvar: String,
    dry_run: bool,
}
