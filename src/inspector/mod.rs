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
#[cfg(test)]
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
