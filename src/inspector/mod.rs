use std::error::Error;
use std::fmt::{self, Display};
use domain_spec::DomainSpec;

pub trait Inspect {
    fn enumerate(&mut self, container_names: &mut Vec<String>) -> Result<(), InspectionError>;
    fn inspect(&mut self, container_name: &str) -> Result<Inspection, InspectionError>;
}

#[derive(Debug, Clone)]
pub struct Inspection {
    pub host: String,
    pub specs: Vec<DomainSpec>,
    pub envvar_present: bool,
}

pub mod docker;
pub mod mock_inspector;

// ############### INSPECTION ERROR #######################

#[derive(Debug)]
pub struct InspectionError {
    inner: Box<Error>,
}

impl Error for InspectionError {
    fn description(&self) -> &str {
        self.inner.description()
    }
    fn cause(&self) -> Option<&Error> {
        Some(&*self.inner)
    }
}

impl Display for InspectionError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "Inspection error. {}", self.inner)
    }
}

pub trait InspectionInnerError: Error {}

impl<T: InspectionInnerError + 'static> From<T> for InspectionError {
    fn from(val: T) -> InspectionError {
        InspectionError { inner: Box::new(val) }
    }
}
