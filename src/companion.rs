
use std;
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use std::cmp::Ordering;
use std::rc::Rc;

use log::LogLevel;
use chan;
use chan_signal::Signal;

use inspector::{Inspect, Inspection, InspectionError};
use publisher::{Publication, PublishingError, Publish};
use common::{Config, MissingEnvVarHandling, MissingContainerHandling};

struct Context {
    pub config: Arc<Config>,
    pub inspector: Box<Inspect>,
    pub publisher: Box<Publish>,
    pub termination_signal: chan::Receiver<Signal>,
}

impl Context {
    fn new(config: Arc<Config>,
           inspector: Box<Inspect>,
           publisher: Box<Publish>,
           termination_signal: chan::Receiver<Signal>)
           -> Context {
        Context {
            config: config,
            termination_signal: termination_signal,
            inspector: inspector,
            publisher: publisher,
        }
    }

    fn inspect(&mut self,
               container_name: Pending<Rc<String>>)
               -> Result<Pending<Inspection>, CompanionError> {
        container_name.try_map(|name| self.inspector.inspect(&name)).map_err(From::from)
    }

    fn publish(&mut self, publication: Publication) -> Result<(), CompanionError> {
        try!(self.publisher.publish(&publication));
        Ok(())
    }

    fn enumerate(&mut self,
                 explicit_container_names: &[Rc<String>])
                 -> (Vec<Pending<Rc<String>>>, Result<(), CompanionError>) {
        let mut container_index: HashMap<Rc<String>, Pending<Rc<String>>> = HashMap::new();
        // Add explicitly listed containers
        for name in explicit_container_names {
            let key: Rc<String> = name.clone();
            let _ = container_index.insert(key,
                                           Pending {
                                               explicit: true,
                                               todo: name.clone(),
                                           });
        }

        // Add enumerated containers
        let enum_result;
        if self.config.enumerate {
            debug!("Enumerating containers");
            let mut enumeration = Vec::new();
            if let Err(e) = self.inspector.enumerate(&mut enumeration) {
                debug!(concat!("Enumeration failed. Program will continue but the following ",
                               "error will be returned: {}"),
                       e);
                enum_result = Err(e)
            } else {
                enum_result = Ok(());
                for name in enumeration.drain(..) {
                    let boxed_name = Rc::new(name);
                    let key = boxed_name.clone();
                    container_index.entry(key).or_insert(Pending {
                        explicit: false,
                        todo: boxed_name,
                    });
                }
            }
        } else {
            enum_result = Ok(())
        }

        let final_names = container_index.drain().map(|kv| kv.1).collect();
        (final_names, enum_result.map_err(|e| From::from(e)))
    }

    /// Wait for the next refresh. Returns true when we should continue with another refresh;
    /// Returns false when we should exit (either because we are on one-shot mode or because
    /// termination was requested)
    fn wait(&mut self) -> bool {
        if let Some(refresh_seconds) = self.config.refresh_seconds {
            let rsig = &mut self.termination_signal;
            debug!("Waiting for {} seconds", refresh_seconds);
            let timeout_duration = std::time::Duration::from_secs(refresh_seconds as u64);
            let refresh_timeout = chan::after(timeout_duration);
            let do_continue: bool;
            chan_select! {
                rsig.recv() -> sig => {
                    debug!("Received {:?} signal. Shutting down.", sig);
                    do_continue = false
                },
                refresh_timeout.recv() => {
                    // just continue with the loop
                    do_continue = true
                },
            };
            do_continue
        } else {
            // Only refresh once and then exit.
            debug!("Refresh disabled. Shutting down.");
            false
        }
    }
}

fn to_publication(inspection: Pending<Inspection>) -> Publication {
    Publication {
        host: inspection.todo.host,
        specs: inspection.todo.specs,
    }
}

pub fn run(config: Arc<Config>,
           inspector: Box<Inspect>,
           publisher: Box<Publish>,
           termination_signal: chan::Receiver<Signal>,
           explicit_container_names: &Vec<Rc<String>>)
           -> Result<(), Vec<CompanionError>> {
    let mut ctx = Context::new(config.clone(), inspector, publisher, termination_signal);
    info!("Companion initialized.");

    loop {
        debug!("Start iteration.");

        // Errors that occurred in this iteration.
        let mut errors = Vec::new();

        // Combine explicitly listed names with containers obtained from enumeration.
        let names = {
            let (names, enum_result) = ctx.enumerate(explicit_container_names);
            if let Err(e) = enum_result {
                errors.push(e)
            }
            names
        };
        debug!("Enumerated containers: {:#?}", names);

        // Refresh each of the containers.
        for name in names.into_iter() {
            refresh_container(name, &mut errors, &mut ctx);
        }

        // Wait for refresh timeout or external abort (kill signal).
        // Returns immediately if we are only supposed to run once.
        if ctx.wait() {
            // We only return the errors from the last iteration. All errors have been logged.
            errors.clear();
        } else {
            // Return errors from the last iteration. This is mainly useful for the case where
            // we only run once. Lets the tool set an appropriate status code on program exit.
            if errors.is_empty() {
                return Ok(());
            } else {
                return Err(errors);
            }
        }
    }
}

/// Inspect and publish updates for the indicated container.
/// If errors happen along the way it will primarily be reported to the log.
/// Errors that are considered 'problematic' (according to configuration) will *additionally*
/// be appended to the `errors` list.
/// Unless you are interested whether a *particular* refresh was successful, you don't need
/// to do anything with these error values (they have already been logged).
fn refresh_container(name: Pending<Rc<String>>, errors: &mut Vec<CompanionError>,
                     ctx: &mut Context) {
    let current_container = name.todo.clone();
    let was_explicit = name.explicit;
    let config = ctx.config.clone();

    // Retrieve requested configuration from the container.
    debug!("Inspect {}", current_container);
    let inspection = match ctx.inspect(name) {
        // Depending on how the companion is configured, an inspection error has different
        // consequences.
        Err(e) => {
            let level;
            let consider_error;
            if config.missing_container == MissingContainerHandling::Report {
                level = LogLevel::Error;
                consider_error = true
            } else if was_explicit {
                level = LogLevel::Warn;
                consider_error = true
            } else {
                level = LogLevel::Info;
                consider_error = false
            }
            log!(level,
                 "Failed to inspect {}. Skipping. Error: {}",
                 current_container,
                 e);
            if consider_error {
                errors.push(e)
            }

            // Need to skip the update for this container (inspection failed)
            return;
        }
        Ok(x) => x,
    };

    // Handle missing env var
    if !inspection.todo.envvar_present {
        let level;
        match (was_explicit, config.missing_envvar) {
            (true, MissingEnvVarHandling::Automatic) |
            (_, MissingEnvVarHandling::Report) => {
                level = LogLevel::Error;
                errors.push(CompanionError::EnvVarMissing(current_container.clone(),
                                                          config.envvar.to_owned()))
            }
            (_, _) => level = LogLevel::Info,
        }
        log!(level,
             "No environment variable '{}' configured for container {}. Skipping.",
             config.envvar,
             current_container);
        return;
    }

    // Publish updated configuration
    let publication = to_publication(inspection);

    if config.dry_run {
        info!("DRY RUN: would update {} with {:#?}",
              current_container,
              publication)
    } else {
        info!("Updating configuration for container {}. Publishing {:?}",
              current_container,
              publication);
        if let Err(e) = ctx.publish(publication) {
            error!("Failed to publish updated configuration for container '{}'. Error: {}",
                   current_container,
                   e);
            errors.push(e);
        }
    }
}


/// Thing that needs to be handled annotated with whether it was requested explicitly or discovered
/// on a best-effort basis. (Affects behaviour in the case of errors)
#[derive(Debug)]
struct Pending<T> {
    /// Whether the container was listed explicitly (changes response to certain error conditions)
    explicit: bool,
    /// The thing that needs to be done
    todo: T,
}

impl<T> Pending<T> {
    #[allow(dead_code)]
    pub fn map<R, F: FnOnce(T) -> R>(self, f: F) -> Pending<R> {
        Pending {
            explicit: self.explicit,
            todo: f(self.todo),
        }
    }
    pub fn try_map<R, E, F: FnOnce(T) -> Result<R, E>>(self, f: F) -> Result<Pending<R>, E> {
        let explicit = self.explicit;
        f(self.todo).map(|t| {
            Pending {
                explicit: explicit,
                todo: t,
            }
        })
    }
}

impl<T: PartialOrd> PartialOrd for Pending<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.todo.partial_cmp(&other.todo)
    }
}

impl<T: PartialEq> PartialEq for Pending<T> {
    fn eq(&self, other: &Self) -> bool {
        self.todo.eq(&other.todo)
    }
}

impl<T: Eq> Eq for Pending<T> {}
impl<T: Ord> Ord for Pending<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.todo.cmp(&other.todo)
    }
}

// ############### COMPANION  ERROR #######################

quick_error! {
    #[derive(Debug)]
    pub enum CompanionError {
        Inspection(err: InspectionError) {
            description("Error during inspection.")
            cause(err)
            from()
            display(me) -> ("{} Error: {}", me.description(), err)
        }
        Publishing(err: PublishingError) {
            description("Error during publishing.")
            cause(err)
            from()
            display(me) -> ("{} Error: {}", me.description(), err)
        }
        EnvVarMissing(container_name: Rc<String>, envvar: Rc<String>) {
            description("Configured environment variable missing on container.")
            display(err) -> ("{} container name: {}, environment variable: {}",
                err.description(), container_name, envvar)
        }
    }
}

// ############### TESTS ##################################
#[cfg(test)]
#[allow(unused_variables, unused_imports)]
mod tests {
    use std::sync::Arc;
    use std::rc::Rc;
    use std::cell::RefCell;
    use std::ops::Deref;

    use chan_signal::Signal;
    use chan;

    use super::*;
    use super::{Context, Pending, refresh_container};
    use common::{self, Config, MissingEnvVarHandling, MissingContainerHandling};
    use ::inspector::mock_inspector::{MockInspector, FakeError};
    use ::inspector::Inspection;
    use ::domain_spec::DomainSpec;
    use ::publisher::mock_publisher::{MockPublisher, MockError};

    #[test]
    fn empty() {
        common::init_log();
        // #### GIVEN ####
        let cfg = Arc::new(Config::default());
        let (term_send, term_recv) = chan::sync(1);

        // #### WHEN  ####
        let ctx = Context::new(cfg,
                               Box::new(MockInspector::default()),
                               Box::new(MockPublisher::default()),
                               term_recv);

        // #### THEN  ####
        // no panic
    }

    #[test]
    fn wait_oneshot() {
        common::init_log();
        // #### GIVEN ####
        let (term_send, term_recv) = chan::sync(1);
        let mut cfg = Config::default();
        cfg.refresh_seconds = None;
        let mut ctx = Context::new(Arc::new(cfg),
                               Box::new(MockInspector::default()),
                               Box::new(MockPublisher::default()),
                               term_recv);

        // #### WHEN  ####
        let do_continue = ctx.wait();

        // #### THEN  ####
        assert!(!do_continue, "One shot companion context tried to run more than once.");
    }

    #[test]
    fn wait_terminate_int() {
        common::init_log();
        // #### GIVEN ####
        let (term_send, term_recv) = chan::sync(1);
        let mut cfg = Config::default();
        cfg.refresh_seconds = Some(1);
        let mut ctx = Context::new(Arc::new(cfg),
                                   Box::new(MockInspector::default()),
                                   Box::new(MockPublisher::default()),
                                   term_recv);

        // #### WHEN  ####
        term_send.send(Signal::INT);
        let do_continue = ctx.wait();

        // #### THEN  ####
        assert!(!do_continue, concat!("Companion context tried to run after ",
                                              "termination was requested."));
    }

    #[test]
    fn wait_terminate_term() {
        common::init_log();
        // #### GIVEN ####
        let (term_send, term_recv) = chan::sync(1);
        let mut cfg = Config::default();
        cfg.refresh_seconds = Some(1);
        let mut ctx = Context::new(Arc::new(cfg),
                                   Box::new(MockInspector::default()),
                                   Box::new(MockPublisher::default()),
                                   term_recv);

        // #### WHEN  ####
        term_send.send(Signal::TERM);
        let do_continue = ctx.wait();

        // #### THEN  ####
        assert!(!do_continue, concat!("Companion context tried to run after ",
                                              "termination was requested."));
    }

    #[test]
    fn wait_1_sec() {
        common::init_log();
        // #### GIVEN ####
        let (term_send, term_recv) = chan::sync(1);
        let mut cfg = Config::default();
        cfg.refresh_seconds = Some(1);
        let mut ctx = Context::new(Arc::new(cfg),
                                   Box::new(MockInspector::default()),
                                   Box::new(MockPublisher::default()),
                                   term_recv);

        // #### WHEN  ####
        let do_continue = ctx.wait();

        // #### THEN  ####
        assert!(do_continue, "Refresh should be successful.");
    }

    #[test]
    fn enumerate_explicit_only() {
        common::init_log();
        // #### GIVEN ####
        let (term_send, term_recv) = chan::sync(1);
        let mut cfg = Config::default();
        cfg.enumerate = false;
        let mut ctx = Context::new(Arc::new(cfg),
                                   Box::new(MockInspector::default()),
                                   Box::new(MockPublisher::default()),
                                   term_recv);
        let alpha = Rc::new("alpha".to_owned());
        let beta = Rc::new("beta".to_owned());
        let explicit_containers = vec![alpha.clone(), beta.clone()];

        // #### WHEN  ####
        let (pendings, result) = ctx.enumerate(&explicit_containers);

        // #### THEN  ####
        assert!(result.is_ok(), "Enumeration result should be Ok(())");
        assert_eq!(pendings.len(), 2);
        assert_eq!(pendings[0].explicit, true);
        assert_eq!(pendings[1].explicit, true);
        assert!(pendings.iter().any(|p| p.todo == alpha), "{} not found in {:?}", alpha, pendings);
        assert!(pendings.iter().any(|p| p.todo == beta), "{} not found in {:?}", beta, pendings);
    }

    #[test]
    fn enumerate_implicit_only() {
        common::init_log();
        // #### GIVEN ####
        let (term_send, term_recv) = chan::sync(1);
        let mut cfg = Config::default();
        cfg.enumerate = true;
        let mut inspector = MockInspector::default();
        let alpha = Rc::new("alpha".to_owned());
        let beta = Rc::new("beta".to_owned());
        inspector.enumerate_result = Ok(vec![(*alpha).clone(), (*beta).clone()]);
        let mut ctx = Context::new(Arc::new(cfg),
                                   Box::new(inspector),
                                   Box::new(MockPublisher::default()),
                                   term_recv);
        let explicit_containers = Vec::new();

        // #### WHEN  ####
        let (pendings, result) = ctx.enumerate(&explicit_containers);

        // #### THEN  ####
        assert!(result.is_ok(), "Enumeration result should be Ok(())");
        assert_eq!(pendings.len(), 2);
        assert_eq!(pendings[0].explicit, false);
        assert_eq!(pendings[1].explicit, false);
        assert!(pendings.iter().any(|p| p.todo == alpha), "{} not found in {:?}", alpha, pendings);
        assert!(pendings.iter().any(|p| p.todo == beta), "{} not found in {:?}", beta, pendings);
    }

    #[test]
    fn enumerate_and_explicit() {
        common::init_log();
        // #### GIVEN ####
        let (term_send, term_recv) = chan::sync(1);
        let mut cfg = Config::default();
        cfg.enumerate = true;
        let mut inspector = MockInspector::default();
        let alpha = Rc::new("alpha".to_owned());
        let beta = Rc::new("beta".to_owned());
        inspector.enumerate_result = Ok(vec![(*alpha).clone(), (*beta).clone()]);
        let mut ctx = Context::new(Arc::new(cfg),
                                   Box::new(inspector),
                                   Box::new(MockPublisher::default()),
                                   term_recv);
        let gamma = Rc::new("gamma".to_owned());
        let delta = Rc::new("delta".to_owned());
        let explicit_containers = vec![gamma.clone(), delta.clone()];

        // #### WHEN  ####
        let (pendings, result) = ctx.enumerate(&explicit_containers);

        // #### THEN  ####
        assert!(result.is_ok(), "Enumeration result should be Ok(())");
        assert_eq!(pendings.len(), 4);
        assert!(pendings.iter().any(|p| p.todo == alpha), "{} not found in {:?}", alpha, pendings);
        assert!(pendings.iter().any(|p| p.todo == beta), "{} not found in {:?}", beta, pendings);
        assert!(pendings.iter().any(|p| p.todo == gamma), "{} not found in {:?}", gamma, pendings);
        assert!(pendings.iter().any(|p| p.todo == delta), "{} not found in {:?}", delta, pendings);
        for pending in pendings {
            if explicit_containers.contains(&pending.todo) {
                assert!(pending.explicit, "Pending item {} expected to be explicit.", pending.todo);
            } else {
                assert!(!pending.explicit, "Pending item {} expected to be explicit.",
                    pending.todo);
            }
        }
    }

    #[test]
    fn enumerate_implicit_fail() {
        common::init_log();
        // #### GIVEN ####
        let (term_send, term_recv) = chan::sync(1);
        let mut cfg = Config::default();
        cfg.enumerate = true;
        let mut inspector = MockInspector::default();
        inspector.enumerate_result = Err(Box::new(|| From::from(FakeError)));
        let mut ctx = Context::new(Arc::new(cfg),
                                   Box::new(inspector),
                                   Box::new(MockPublisher::default()),
                                   term_recv);
        let alpha = Rc::new("alpha".to_owned());
        let beta = Rc::new("beta".to_owned());
        let explicit_containers = vec![alpha.clone(), beta.clone()];

        // #### WHEN  ####
        let (pendings, result) = ctx.enumerate(&explicit_containers);

        // #### THEN  ####
        assert_eq!(pendings.len(), 2);
        assert_eq!(pendings[0].explicit, true);
        assert_eq!(pendings[1].explicit, true);
        assert!(pendings.iter().any(|p| p.todo == alpha), "{} not found in {:?}", alpha, pendings);
        assert!(pendings.iter().any(|p| p.todo == beta), "{} not found in {:?}", beta, pendings);

        assert!(result.is_err(), "Enumeration result should be Err(_), was {:?}", result);
        if let Err(CompanionError::Inspection(err)) = result {
            assert!(format!("{:?}", err).contains("Fake"), "Expected fake error.");
        } else {
            assert!(false, "Expected inspection error, got {:?} instead.", result);
        }
    }

    #[test]
    fn refresh_explicit_success() {
        common::init_log();
        // #### GIVEN ####
        // test configuration
        let cfg = Config::default();

        // mock publisher
        let publisher = Arc::new(RefCell::new(MockPublisher::default()));

        // mock inspector
        let beta = Rc::new("beta".to_owned());
        let mut inspector = MockInspector::default();
        let spec1 = DomainSpec { domain_name: "one.beta.domain".to_owned(),
            http_port: Some(80),
            https_port: Some(443) };
        let spec2 = DomainSpec { domain_name: "two.beta.domain".to_owned(),
            http_port: Some(8080),
            https_port: None };
        inspector.inspect_results.insert(beta.clone(), Ok(Inspection {
            envvar_present: true,
            host: "beta.host".to_owned(),
            specs: vec![spec1.clone(), spec2.clone()]
        }));

        // companion context
        let (term_send, term_recv) = chan::sync(1);
        let mut ctx = Context::new(Arc::new(cfg),
            Box::new(inspector),
            Box::new(publisher.clone()),
            term_recv);
        let mut errors = Vec::new();

        // #### WHEN  ####
        refresh_container(Pending { todo: beta, explicit: true }, &mut errors, &mut ctx);

        // #### THEN  ####
        assert!(errors.len() == 0, "Expected no errors, got {:#?}", errors);
        {
            let ref publisher_cell = &*publisher;
            let publisher_cell_ref = publisher_cell.borrow();
            let mock : &MockPublisher = publisher_cell_ref.deref();

            assert!(mock.publications.iter().all(|p| p.host == "beta.host"),
                "Host expected to be 'alpha.host' for all publications. List: {:#?}",
                mock.publications);
            assert!(mock.publications.iter().any(|p| p.specs.iter().any(|s| *s == spec1)),
                "Expected 'spec1' to be published.\nSpec1: {:#?}\nList: {:#?}",
                spec1, mock.publications);
            assert!(mock.publications.iter().any(|p| p.specs.iter().any(|s| *s == spec2)),
                "Expected 'spec2' to be published.\nSpec2: {:#?}\nList: {:#?}",
                spec1, mock.publications);
        }
    }

    #[test]
    fn refresh_dry_run() {
        common::init_log();
        // #### GIVEN ####
        // test configuration
        let mut cfg = Config::default();
        cfg.dry_run = true;

        // mock publisher
        let publisher = Arc::new(RefCell::new(MockPublisher::default()));
        publisher.borrow_mut().error_trigger =
            Some(("domain".to_owned(), Box::new(|| From::from(MockError))));

        // mock inspector
        let alpha = Rc::new("alpha".to_owned());
        let beta = Rc::new("beta".to_owned());
        let mut inspector = MockInspector::default();
        let spec1 = DomainSpec { domain_name: "one.alpha.domain".to_owned(),
            http_port: Some(80),
            https_port: Some(443) };
        let spec2 = DomainSpec { domain_name: "two.beta.domain".to_owned(),
            http_port: Some(8080),
            https_port: None };
        inspector.inspect_results.insert(beta.clone(), Ok(Inspection {
            envvar_present: true,
            host: "beta.host".to_owned(),
            specs: vec![spec1.clone()]
        }));
        inspector.inspect_results.insert(alpha.clone(), Ok(Inspection {
            envvar_present: true,
            host: "alpha.host".to_owned(),
            specs: vec![spec2.clone()]
        }));

        // companion context
        let (term_send, term_recv) = chan::sync(1);
        let mut ctx = Context::new(Arc::new(cfg),
                                   Box::new(inspector),
                                   Box::new(publisher.clone()),
                                   term_recv);
        let mut errors = Vec::new();

        // #### WHEN  ####
        refresh_container(Pending { todo: beta, explicit: true }, &mut errors, &mut ctx);

        // #### THEN  ####
        assert!(errors.len() == 0, "Expected no errors, got {:#?}", errors);
        assert!(publisher.borrow().publications.len() == 0,
            "Dry run shouldn't trigger publications. Got {:#?}",
            publisher.borrow().publications);
    }

    #[test]
    fn refresh_fail_publish() {
        common::init_log();
        // #### GIVEN ####
        // test configuration
        let cfg = Config::default();

        // mock publisher
        let publisher = Arc::new(RefCell::new(MockPublisher::default()));
        publisher.borrow_mut().error_trigger =
            Some(("domain".to_owned(), Box::new(|| From::from(MockError))));

        // mock inspector
        let alpha = Rc::new("alpha".to_owned());
        let beta = Rc::new("beta".to_owned());
        let mut inspector = MockInspector::default();
        let spec1 = DomainSpec { domain_name: "one.alpha.domain".to_owned(),
            http_port: Some(80),
            https_port: Some(443) };
        let spec2 = DomainSpec { domain_name: "two.beta.domain".to_owned(),
            http_port: Some(8080),
            https_port: None };
        inspector.inspect_results.insert(beta.clone(), Ok(Inspection {
            envvar_present: true,
            host: "beta.host".to_owned(),
            specs: vec![spec1.clone()]
        }));
        inspector.inspect_results.insert(alpha.clone(), Ok(Inspection {
            envvar_present: true,
            host: "alpha.host".to_owned(),
            specs: vec![spec2.clone()]
        }));

        // companion context
        let (term_send, term_recv) = chan::sync(1);
        let mut ctx = Context::new(Arc::new(cfg),
                                   Box::new(inspector),
                                   Box::new(publisher.clone()),
                                   term_recv);
        let mut errors = Vec::new();

        // #### WHEN  ####
        refresh_container(Pending { todo: beta, explicit: true }, &mut errors, &mut ctx);

        // #### THEN  ####
        assert!(errors.len() > 0, "Expected some errors, got {:#?}", errors);
        assert!(errors.iter().any(|e| format!("{:?}", e).contains("Mock")));
        assert!(publisher.borrow().publications.len() == 0, "Unexpected publications: {:#?}",
            publisher.borrow().publications);
    }

    #[test]
    fn refresh_fail_envvar_automatic_explicit() {
        common::init_log();
        // #### GIVEN ####
        // test configuration
        let mut cfg = Config::default();
        cfg.missing_envvar = MissingEnvVarHandling::Automatic;

        // mock publisher
        let publisher = Arc::new(RefCell::new(MockPublisher::default()));

        // mock inspector
        let alpha = Rc::new("alpha".to_owned());
        let beta = Rc::new("beta".to_owned());
        let mut inspector = MockInspector::default();
        let spec1 = DomainSpec { domain_name: "one.alpha.domain".to_owned(),
            http_port: Some(80),
            https_port: Some(443) };
        let spec2 = DomainSpec { domain_name: "two.beta.domain".to_owned(),
            http_port: Some(8080),
            https_port: None };
        inspector.inspect_results.insert(beta.clone(), Ok(Inspection {
            envvar_present: false,
            host: "beta.host".to_owned(),
            specs: Vec::new()
        }));
        inspector.inspect_results.insert(alpha.clone(), Ok(Inspection {
            envvar_present: true,
            host: "alpha.host".to_owned(),
            specs: vec![spec2.clone()]
        }));

        // companion context
        let (term_send, term_recv) = chan::sync(1);
        let mut ctx = Context::new(Arc::new(cfg),
                                   Box::new(inspector),
                                   Box::new(publisher.clone()),
                                   term_recv);
        let mut errors = Vec::new();

        // #### WHEN  ####
        refresh_container(Pending { todo: beta, explicit: true }, &mut errors, &mut ctx);

        // #### THEN  ####
        assert!(errors.len() > 0, "Expected some errors, got {:#?}", errors);
        assert!(errors.iter().any(|e| format!("{:?}", e).contains("EnvVarMissing")));
        assert!(publisher.borrow().publications.len() == 0, "Unexpected publications: {:#?}",
            publisher.borrow().publications);
    }

    #[test]
    fn refresh_fail_envvar_automatic_implicit() {
        common::init_log();
        // #### GIVEN ####
        // test configuration
        let mut cfg = Config::default();
        cfg.missing_envvar = MissingEnvVarHandling::Automatic;

        // mock publisher
        let publisher = Arc::new(RefCell::new(MockPublisher::default()));

        // mock inspector
        let alpha = Rc::new("alpha".to_owned());
        let beta = Rc::new("beta".to_owned());
        let mut inspector = MockInspector::default();
        let spec1 = DomainSpec { domain_name: "one.alpha.domain".to_owned(),
            http_port: Some(80),
            https_port: Some(443) };
        let spec2 = DomainSpec { domain_name: "two.beta.domain".to_owned(),
            http_port: Some(8080),
            https_port: None };
        inspector.inspect_results.insert(beta.clone(), Ok(Inspection {
            envvar_present: false,
            host: "beta.host".to_owned(),
            specs: Vec::new()
        }));
        inspector.inspect_results.insert(alpha.clone(), Ok(Inspection {
            envvar_present: true,
            host: "alpha.host".to_owned(),
            specs: vec![spec2.clone()]
        }));

        // companion context
        let (term_send, term_recv) = chan::sync(1);
        let mut ctx = Context::new(Arc::new(cfg),
                                   Box::new(inspector),
                                   Box::new(publisher.clone()),
                                   term_recv);
        let mut errors = Vec::new();

        // #### WHEN  ####
        refresh_container(Pending { todo: beta, explicit: false }, &mut errors, &mut ctx);

        // #### THEN  ####
        // This time, the inspection error shouldn't be treated as something serious
        assert!(errors.len() == 0, "Expected no errors, got {:#?}", errors);
        assert!(publisher.borrow().publications.len() == 0, "Unexpected publications: {:#?}",
            publisher.borrow().publications);
    }

    #[test]
    fn refresh_fail_envvar_ignore() {
        common::init_log();
        // #### GIVEN ####
        // test configuration
        let mut cfg = Config::default();
        cfg.missing_envvar = MissingEnvVarHandling::Ignore;

        // mock publisher
        let publisher = Arc::new(RefCell::new(MockPublisher::default()));

        // mock inspector
        let alpha = Rc::new("alpha".to_owned());
        let beta = Rc::new("beta".to_owned());
        let mut inspector = MockInspector::default();
        let spec1 = DomainSpec { domain_name: "one.alpha.domain".to_owned(),
            http_port: Some(80),
            https_port: Some(443) };
        let spec2 = DomainSpec { domain_name: "two.beta.domain".to_owned(),
            http_port: Some(8080),
            https_port: None };
        inspector.inspect_results.insert(beta.clone(), Ok(Inspection {
            envvar_present: false,
            host: "beta.host".to_owned(),
            specs: Vec::new()
        }));
        inspector.inspect_results.insert(alpha.clone(), Ok(Inspection {
            envvar_present: true,
            host: "alpha.host".to_owned(),
            specs: vec![spec2.clone()]
        }));

        // companion context
        let (term_send, term_recv) = chan::sync(1);
        let mut ctx = Context::new(Arc::new(cfg),
                                   Box::new(inspector),
                                   Box::new(publisher.clone()),
                                   term_recv);
        let mut errors = Vec::new();

        // #### WHEN  ####
        refresh_container(Pending { todo: beta, explicit: true }, &mut errors, &mut ctx);

        // #### THEN  ####
        // This time, the inspection error shouldn't be treated as something serious
        assert!(errors.len() == 0, "Expected no errors, got {:#?}", errors);
        assert!(publisher.borrow().publications.len() == 0, "Unexpected publications: {:#?}",
            publisher.borrow().publications);
    }

    #[test]
    fn refresh_fail_inspect_implicit_report() {
        common::init_log();
        // #### GIVEN ####
        // test configuration
        let mut cfg = Config::default();
        cfg.missing_container = MissingContainerHandling::Report;

        // mock publisher
        let publisher = Arc::new(RefCell::new(MockPublisher::default()));

        // mock inspector
        let alpha = Rc::new("alpha".to_owned());
        let beta = Rc::new("beta".to_owned());
        let mut inspector = MockInspector::default();
        let spec1 = DomainSpec { domain_name: "one.alpha.domain".to_owned(),
            http_port: Some(80),
            https_port: Some(443) };
        let spec2 = DomainSpec { domain_name: "two.beta.domain".to_owned(),
            http_port: Some(8080),
            https_port: None };
        inspector.inspect_results.insert(beta.clone(), Err(Box::new(|| From::from(FakeError))));
        inspector.inspect_results.insert(alpha.clone(), Ok(Inspection {
            envvar_present: true,
            host: "alpha.host".to_owned(),
            specs: vec![spec2.clone()]
        }));

        // companion context
        let (term_send, term_recv) = chan::sync(1);
        let mut ctx = Context::new(Arc::new(cfg),
                                   Box::new(inspector),
                                   Box::new(publisher.clone()),
                                   term_recv);
        let mut errors = Vec::new();

        // #### WHEN  ####
        refresh_container(Pending { todo: beta, explicit: false }, &mut errors, &mut ctx);

        // #### THEN  ####
        assert!(errors.len() > 0, "Expected some errors, got {:#?}", errors);
        assert!(errors.iter().any(|e| format!("{:?}", e).contains("Fake")));
        assert!(publisher.borrow().publications.len() == 0, "Unexpected publications: {:#?}",
            publisher.borrow().publications);
    }

    #[test]
    fn refresh_fail_inspect_implicit_ignore() {
        common::init_log();
        // #### GIVEN ####
        // test configuration
        let mut cfg = Config::default();
        cfg.missing_container = MissingContainerHandling::Ignore;

        // mock publisher
        let publisher = Arc::new(RefCell::new(MockPublisher::default()));

        // mock inspector
        let alpha = Rc::new("alpha".to_owned());
        let beta = Rc::new("beta".to_owned());
        let mut inspector = MockInspector::default();
        let spec1 = DomainSpec { domain_name: "one.alpha.domain".to_owned(),
            http_port: Some(80),
            https_port: Some(443) };
        let spec2 = DomainSpec { domain_name: "two.beta.domain".to_owned(),
            http_port: Some(8080),
            https_port: None };
        inspector.inspect_results.insert(beta.clone(), Err(Box::new(|| From::from(FakeError))));
        inspector.inspect_results.insert(alpha.clone(), Ok(Inspection {
            envvar_present: true,
            host: "alpha.host".to_owned(),
            specs: vec![spec2.clone()]
        }));

        // companion context
        let (term_send, term_recv) = chan::sync(1);
        let mut ctx = Context::new(Arc::new(cfg),
                                   Box::new(inspector),
                                   Box::new(publisher.clone()),
                                   term_recv);
        let mut errors = Vec::new();

        // #### WHEN  ####
        refresh_container(Pending { todo: beta, explicit: false }, &mut errors, &mut ctx);

        // #### THEN  ####
        assert!(errors.len() == 0, "Expected no errors, got {:#?}", errors);
        assert!(publisher.borrow().publications.len() == 0, "Unexpected publications: {:#?}",
            publisher.borrow().publications);
    }

    /// Normally, DomainSpec isn't directly comparable because instances might not be in canonical
    /// form, but for testing, this is good enough.
    impl PartialEq for DomainSpec {
        fn eq(&self, other: &DomainSpec) -> bool {
            self.domain_name == other.domain_name
                && self.http_port == other.http_port
                && self.https_port == other.https_port
        }
    }
    impl Eq for DomainSpec {}
}