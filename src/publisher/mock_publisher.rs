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

use std::fmt::{self, Debug, Display};
use std::default::Default;
use std::cell::RefCell;
use std::sync::Arc;
use std::error::Error;

use super::{Publication, PublishingError, PublishingInnerError, Publish};

pub struct MockPublisher {
    pub publications: Vec<Publication>,
    pub error_trigger: Option<(String, Box<Fn() -> PublishingError>)>,
}

impl Default for MockPublisher {
    fn default() -> MockPublisher {
        MockPublisher { error_trigger: None, publications: Vec::new() }
    }
}

impl Debug for MockPublisher {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,
               "MockPublisher {{ publications: {:?}, error_trigger: {:?} }}",
               self.publications,
               self.error_trigger.as_ref().map(|p| {
                   let &(ref k, _) = p;
                   (k, "*")
               }))
    }
}

impl Publish for MockPublisher {
    fn publish(&mut self, publication: &Publication) -> Result<(), PublishingError> {
        if let Some((ref trigger, ref error)) = self.error_trigger {
            let specs = &publication.specs;
            if specs.into_iter().any(|spec| spec.domain_name.contains(trigger)) {
                return Err(error());
            }
        }
        self.publications.push(publication.clone());
        Ok(())
    }
}

/// Runtime checked reference to allow a mock publisher to be inspected even after it has been
/// handed over. Panics if it cannot perform a mutable borrow.
impl Publish for Arc<RefCell<MockPublisher>> {
    fn publish(&mut self, publication: &Publication) -> Result<(), PublishingError> {
        (*self).borrow_mut().publish(publication)
    }
}

#[derive(Debug,Clone,Eq,PartialEq)]
pub struct MockError;
impl Error for MockError {
    fn description(&self) -> &str {
        "Mock error"
    }
}
impl Display for MockError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}
impl PublishingInnerError for MockError {}
