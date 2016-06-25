use std::sync::Arc;
use std::fmt::{self, Display, Debug};
use std::error::Error;
use std::convert::From;
use std::collections::HashMap;
use std::rc::Rc;

use common::Config;
use domain_spec::{self, DomainSpec};
use super::*;

pub struct MockInspector {
    pub enumerate_result: Result<Vec<String>, Box<Fn() -> Box<InspectionInnerError>>>,
    pub inspect_results: HashMap<Rc<String>, Result<Inspection, InspectionError>>
}

impl Debug for MockInspector {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        try!(write!(f, "MockInspector {{ enumerate_result: "));
        match self.enumerate_result {
            Ok(ref v) => {
                try!(write!(f, "Ok("));
                try!(Debug::fmt(v, f));
                try!(write!(f,")"));
            },
            Err(_) => {
                try!(write!(f, "Err(*)"));
            }
        }
        try!(write!(f, ", inspect_results: "));
        try!(Debug::fmt(&self.inspect_results,f));
        write!(f, " }}")
    }
}

impl MockInspector {
    pub fn new() -> MockInspector {
        MockInspector { enumerate_result: Ok(Vec::new()), inspect_results: HashMap::new() }
    }
}

impl Default for MockInspector {
    pub fn default() -> MockInspector {
        MockInspector::new()
    }
}

impl Inspect for MockInspector {
    fn enumerate(&mut self, container_names: &mut Vec<String>) -> Result<(), InspectionError> {
        self.enumerate_result.as_ref().map(|v| container_names.extend_from_slice(&v)).map_err(|b| From::from(b()))
    }
    fn inspect(&mut self, container_name: &str) -> Result<Inspection, InspectionError> {
        self.inspect_results.get(container_name)
            .ok_or_else(|| InspectionNotMocked { container_name: container_name.to_string() })
    }
}

#[derive(Debug)]
pub struct InspectionNotMocked {
    pub container_name: String
}

impl Error for InspectionNotMocked {
    fn description(&self) -> &str {
        "No inspection result provided for given container name."
    }
    fn cause(&self) -> Option<&Error> {
        None
    }
}

impl Display for InspectionNotMocked {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "{}. Container name: {}", self.description(), self.container_name)
    }
}

impl InspectionInnerError for InspectionNotMocked {}
