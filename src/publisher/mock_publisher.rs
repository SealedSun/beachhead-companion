
use super::{Publication, PublishingError, Publish};

#[derive(Debug)]
pub struct MockPublisher {
    pub publications: Vec<Publication>,
    pub error_trigger: Option<(String, PublishingError)>
}

impl Publish for MockPublisher {
    fn publish(&mut self, publication: &Publication) -> Result<(), PublishingError> {
        if let Some((trigger, error)) = self.error_trigger {
            if publication.specs.into_iter().any(|spec| spec.domain_name.contains(trigger)) {
                return error
            }
        }
        self.publications.push(publication.clone());
        Ok(())
    }
}
