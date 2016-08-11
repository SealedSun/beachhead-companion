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

/// Abstract interface for the component that publishes the current configuration state to whatever
/// system needs to be informed.
pub trait Publish {
    fn publish(&mut self, publication: &Publication) -> Result<(), PublishingError>;
}

#[derive(Debug, Clone)]
pub struct Publication {
    pub host: String,
    pub specs: Vec<DomainSpec>,
}

pub mod redis;
#[cfg(test)]
pub mod mock_publisher;
mod json_serializer;
#[cfg(test)]
mod test_redis_publisher;

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
