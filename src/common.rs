// The MIT License (MIT)
//
// Copyright (c) 2016 Christian Klauser
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use url::Url;
use std::fmt::Display;
use log;
use std;
use std::io::{stderr, Write};
use std::rc::Rc;

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
    pub redis_host: Rc<String>,
    /// The port of the redis server.
    pub redis_port: u16,
    /// The prefix for the keys to insert into redis. Will be followed by the container name.
    pub key_prefix: Rc<String>,
    /// The expiration for registrations in seconds. None means no expiration.
    pub expire_seconds: Option<u32>,
    /// The refresh interval for registrations in seconds. None means no refresh,
    /// only set once.
    pub refresh_seconds: Option<u32>,
    /// URL to the docker socket.
    pub docker_url: Url,
    /// Whether to use the container hostname (true) or lookup the bridge network IP.
    pub docker_network: bool,
    /// Name of the environment variable to look up in docker container configuration.
    pub envvar: Rc<String>,
    /// Indicates whether this is a dry-run where the Redis update is not performed.
    pub dry_run: bool,
    /// How to handle missing environment variables on containers.
    pub missing_envvar: MissingEnvVarHandling,
    /// How to handle missing containers.
    pub missing_container: MissingContainerHandling,
    /// Instead of (or in addition to) listing containers explicitly, enumerate the containers
    /// running on the docker host. Containers found via enumeration and not listed explicitly are
    /// have slightly different error handling by default.
    pub enumerate: bool,
    /// Whether service manager notifications (READY, WATCHDOG) are enabled.
    pub systemd: bool,
    /// The number of milliseconds a service manager waits between 'alive' pings from this program.
    pub watchdog_microseconds: Option<u64>,
}

/// Behaviour when confronted with a container that does not have a beachhead environment variable
/// set. See enum constants for details.
#[derive(Debug,Eq,PartialEq,Copy,Clone)]
pub enum MissingEnvVarHandling {
    /// Report error for explicitly listed containers. Ignore containers found via enumeration.
    Automatic,
    /// Report missing environment variables as an error for all containers.
    Report,
    /// Simply ignore missing environment variables for all containers.
    Ignore,
}

/// Behaviour when confronted with a container that cannot be inspected. See enum constants for
/// details. The main idea behind this setting is that it's not beachhead companion's job to
/// monitor your containers. If it's not there, don't publish its configuration (let it expire).
#[derive(Debug,Eq,PartialEq,Copy,Clone)]
pub enum MissingContainerHandling {
    /// Report inspection failures as an error
    Report,
    /// Ignore containers that cannot be inspected.
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
            redis_host: Rc::new("localhost".to_owned()),
            redis_port: 6379,
            key_prefix: Rc::new("".to_owned()),
            expire_seconds: Some(60),
            refresh_seconds: Some(27),
            docker_url: Url::parse("unix://var/run/docker.sock").unwrap(),
            docker_network: false,
            envvar: Rc::new("BEACHHEAD_DOMAINS".to_owned()),
            dry_run: false,
            missing_envvar: Default::default(),
            missing_container: Default::default(),
            enumerate: false,
            systemd: false,
            watchdog_microseconds: None,
        }
    }
}

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

/// Turn an optional result into a result of an optional value.
///
/// # Examples
/// ```
/// # use libbeachheadcompanion::common::optional_result;
/// assert_eq!(optional_result::<i32,i32>(Some(Ok(2))),       Ok(Some(2)) );
/// assert_eq!(optional_result::<&str,&str>(Some(Err("fail"))), Err("fail") );
/// assert_eq!(optional_result::<i32,i32>(None),              Ok(None)    );
/// ```
pub fn optional_result<R, E>(x_opt: Option<Result<R, E>>) -> Result<Option<R>, E> {
    match x_opt {
        Some(Ok(x)) => Ok(Some(x)),
        Some(Err(e)) => Err(e),
        None => Ok(None),
    }
}

/// Display errors a bit more nicely, depending on whether logging is enabled or not.
/// This variant accepts Error results with a single error value.
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

/// Display errors a bit more nicely, depending on whether logging is enabled or not.
/// This variant accepts Error results with multiple error values.
pub fn stay_very_calm_and<T, E>(result: Result<T, Vec<E>>)
    where E: Display
{
    match result {
        Ok(_) => (),
        Err(es) => {
            // We need errors to be shown to the user. If we can, we use the error logging
            // mechanism. Otherwise, we just print to stderr.
            if log_enabled!(log::LogLevel::Error) {
                for e in es {
                    error!("Fatal error: {}", e);
                }
            } else {
                for e in es {
                    match writeln!(&mut stderr(), "Fatal error: {}", e) {
                        Err(_) => (), // ignore, nothing left to do
                        Ok(_) => (),
                    }
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

    #[test]
    fn optional_result_full() {
        init_log();
        assert_eq!(optional_result::<&str, &str>(Some(Ok("ok"))), Ok(Some("ok")));
        assert_eq!(optional_result::<&str, &str>(Some(Err("fail"))), Err("fail"));
        assert_eq!(optional_result::<&str, &str>(None), Ok(None));
    }
}
