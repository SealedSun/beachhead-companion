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

use std::fmt::{self, Display, Debug};
use std::error::Error;
use std::convert::From;
use std::collections::HashMap;
use std::rc::Rc;

use super::*;

pub struct MockInspector {
    pub enumerate_result: Result<Vec<String>, Box<Fn() -> InspectionError>>,
    pub inspect_results: HashMap<Rc<String>, Result<Inspection, Box<Fn() -> InspectionError>>>,
}

impl Debug for MockInspector {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        try!(write!(f, "MockInspector {{ enumerate_result: "));
        match self.enumerate_result {
            Ok(ref v) => {
                try!(write!(f, "Ok("));
                try!(Debug::fmt(v, f));
                try!(write!(f, ")"));
            }
            Err(_) => {
                try!(write!(f, "Err(*)"));
            }
        }
        try!(write!(f, ", inspect_results: ["));
        for inspect_result in self.inspect_results.iter() {
            let (k, v) = inspect_result;
            try!(write!(f, " {} => ", k));
            match v {
                &Ok(ref result) => {
                    try!(write!(f, "Ok("));
                    try!(Debug::fmt(result, f));
                    try!(write!(f, ") "));
                }
                _ => {
                    try!(write!(f, "Err(*) "));
                }
            }
        }
        write!(f, "] }}")
    }
}

impl MockInspector {
    pub fn new() -> MockInspector {
        MockInspector { enumerate_result: Ok(Vec::new()), inspect_results: HashMap::new() }
    }
}

impl Default for MockInspector {
    fn default() -> MockInspector {
        MockInspector::new()
    }
}

impl Inspect for MockInspector {
    fn enumerate(&mut self, container_names: &mut Vec<String>) -> Result<(), InspectionError> {
        self.enumerate_result
            .as_ref()
            .map(|v| container_names.extend_from_slice(&v))
            .map_err(|b| b())
    }
    fn inspect(&mut self, container_name: &str) -> Result<Inspection, InspectionError> {
        let my_container_name = Rc::new(container_name.to_owned());
        match self.inspect_results.get(&my_container_name) {
            None => {
                Err(From::from(InspectionNotMocked { container_name: container_name.to_string() }))
            }
            Some(&Ok(ref i)) => Ok(i.clone()),
            Some(&Err(ref f)) => Err(f()),
        }
    }
}

/// Error that gets thrown when there is no mock data for a particular container name.
#[derive(Debug)]
pub struct InspectionNotMocked {
    /// The name of the container that the mock inspector was trying to look up.
    pub container_name: String,
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

impl From<Box<InspectionNotMocked>> for InspectionError {
    fn from(val: Box<InspectionNotMocked>) -> InspectionError {
        InspectionError { inner: Box::new(*val) }
    }
}

/// An error dedicated to test error handling code paths.
#[derive(Debug)]
pub struct FakeError;
impl Error for FakeError {
    fn description(&self) -> &str {
        "Fake error."
    }
    fn cause(&self) -> Option<&Error> {
        None
    }
}
impl Display for FakeError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "{}", self.description())
    }
}
impl InspectionInnerError for FakeError {}
impl From<Box<FakeError>> for InspectionError {
    fn from(val: Box<FakeError>) -> InspectionError {
        InspectionError { inner: Box::new(*val) }
    }
}
