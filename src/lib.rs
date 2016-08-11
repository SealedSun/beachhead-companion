
extern crate regex;
extern crate redis;
extern crate shiplift;
extern crate env_logger;
extern crate chrono;
extern crate rustc_serialize;
extern crate url;
extern crate chan_signal;
extern crate systemd;

#[macro_use]
extern crate log;
#[macro_use]
extern crate chan;
#[macro_use]
extern crate quick_error;
#[macro_use]
extern crate lazy_static;

#[macro_use]
pub mod common;
pub mod domain_spec;
pub mod inspector;
pub mod publisher;
pub mod companion;

pub const VERSION: &'static str = env!("CARGO_PKG_VERSION");
