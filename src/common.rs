use url::Url;
use std::fmt::Display;
use log;
use std;
use std::io::{stderr, Write};

/// This macro is syntactic sugar for passing additional arguments to an error "conversion
/// constructor". The idea is that you define `From<(YourError, Additional, Args)>` (a conversion
/// from a tuple to an error) and then use this macro to supply the additional arguments.
/// ```
/// try_!(produced_your_error(&file_name), file_name.to_owned(), args.clone())
/// ```
/// The additional arguments are of course only evaluated when the guarded expression returns
/// an error.
#[macro_export]
macro_rules! try_ {
    ($expr:expr, $($details:expr),+) => (match $expr {
        ::std::result::Result::Ok(val) => val,
        ::std::result::Result::Err(err) => {
            return ::std::result::Result::Err(
                ::std::convert::From::from((err, ($($details),+) )))
        }
    })
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

pub enum MissingEnvVarHandling {
    Automatic,
    Report,
    Ignore,
}

pub enum MissingContainerHandling {
    Report,
    Ignore,
}

impl Default for MissingEnvVarHandling {
    fn default() -> MissingEnvVarHandling {
        MissingEnvVarHandling::Automatic
    }
}

impl Default for MissingContainerHandling {
    fn default() -> MissingContainerHandling {
        MissingContainerHandling::Ignore
    }
}

// This is just intended as a shorthand for unit testing.
// For the real application, the default configuration is derived from the default Args struct,
// which, in turn, is defined by the docopt USAGE.
#[cfg(test)]
impl Default for Config {
    fn default() -> Config {
        Config {
            redis_host: "localhost".to_owned(),
            redis_port: 6379,
            key_prefix: "".to_owned(),
            expire_seconds: Some(60),
            refresh_seconds: Some(27),
            docker_url: Url::parse("unix://var/run/docker.sock").unwrap(),
            docker_network: false,
            envvar: "BEACHHEAD_DOMAINS".to_owned(),
            dry_run: false,
            missing_envvar: Default::default(),
            missing_container: Default::default(),
        }
    }
}

#[cfg(test)]
pub fn init_log() {
    use env_logger;
    lazy_static! {
        static ref TEST_LOG : bool = {
            env_logger::init().expect("Initialize test logger from env var RUST_LOG");
            true
        };
    }
    if !*TEST_LOG {
        panic!("Failed to set up TEST_LOG!");
    }
}

pub fn optional_result<R, E>(x_opt: Option<Result<R, E>>) -> Result<Option<R>, E> {
    match x_opt {
        Some(Ok(x)) => Ok(Some(x)),
        Some(Err(e)) => Err(e),
        None => Ok(None),
    }
}

/// Display errors a bit more nicely, depending on whether logging is enabled or not.
pub fn stay_calm_and<T, E>(result: Result<T, E>)
    where E: Display
{
    match result {
        Ok(_) => (),
        Err(e) => {
            // We need errors to be shown to the user. If we can, we use the error logging
            // mechanism. Otherwise, we just print to stderr.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        init_log();
        // #### GIVEN ####

        // #### WHEN  ####
        let config: Config = Default::default();

        // #### THEN  ####
        // no panic
        assert!(config.expire_seconds.is_some());
    }
}
