use std::error::Error;
use std::fmt::{self, Display};

use domain_spec::DomainSpec;

/// Abstract interface for the component that publishes the current configuration state to whatever
/// system needs to be informed.
pub trait Publish {
    fn publish(&mut self, publication: &Publication) -> Result<(), PublishingError>;
}

#[derive(Debug)]
pub struct Publication {
    pub host: String,
    pub specs: Vec<DomainSpec>,
}

pub mod redis;
mod json_serializer;

// ############### PUBLISHING ERROR #######################

#[derive(Debug)]
pub struct PublishingError {
    inner: Box<Error>,
}

impl Error for PublishingError {
    fn description(&self) -> &str {
        self.inner.description()
    }
    fn cause(&self) -> Option<&Error> {
        Some(&*self.inner)
    }
}

impl Display for PublishingError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "Publishing error. {}", self.inner)
    }
}

pub trait PublishingInnerError: Error {}

impl<T: PublishingInnerError + 'static> From<T> for PublishingError {
    fn from(val: T) -> PublishingError {
        PublishingError { inner: Box::new(val) }
    }
}
