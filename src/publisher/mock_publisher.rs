
use std::fmt::{self, Debug};

use super::{Publication, PublishingError, Publish};

pub struct MockPublisher {
    pub publications: Vec<Publication>,
    pub error_trigger: Option<(String, Box<Fn() -> PublishingError>)>
}

impl Debug for MockPublisher {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "MockPublisher {{ publications: {:?}, error_trigger: {:?} }}", self.publications,
               self.error_trigger.as_ref().map(|p| { let &(ref k, _) = p; (k, "*") }))
    }
}

impl Publish for MockPublisher {
    fn publish(&mut self, publication: &Publication) -> Result<(), PublishingError> {
        if let Some((ref trigger, ref error)) = self.error_trigger {
            let specs  = &publication.specs;
            if specs.into_iter().any(|spec| spec.domain_name.contains(trigger)) {
                return Err(error())
            }
        }
        self.publications.push(publication.clone());
        Ok(())
    }
}
