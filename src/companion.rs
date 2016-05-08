
use std;
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use std::cmp::Ordering;
use std::rc::Rc;

use log::LogLevel;
use chan;
use chan_signal::Signal;

use ::inspector::{Inspect, Inspection, InspectionError};
use ::publisher::{Publication, PublishingError, Publish};
use ::common::{Config, MissingEnvVarHandling, MissingContainerHandling};

struct Context {
    pub config: Arc<Config>,
    pub inspector: Box<Inspect>,
    pub publisher: Box<Publish>,
    pub termination_signal: chan::Receiver<Signal>
}

impl Context {
    fn new(config: Arc<Config>, inspector: Box<Inspect>, publisher: Box<Publish>,
        termination_signal: chan::Receiver<Signal>) -> Context {
        Context {
            config: config,
            termination_signal: termination_signal,
            inspector: inspector,
            publisher: publisher
        }
    }

    fn inspect(&mut self, container_name: Pending<Rc<String>>) -> Result<Pending<Inspection>, CompanionError> {
        container_name.try_map(|name| self.inspector.inspect(&name)).map_err(From::from)
    }

    fn publish(&mut self, publication: Publication) -> Result<(), CompanionError> {
        try!(self.publisher.publish(&publication));
        Ok(())
    }

    fn enumerate(&mut self, explicit_container_names: &Vec<Rc<String>>) -> (Vec<Pending<Rc<String>>>, Result<(), CompanionError>) {
        let mut container_index : HashMap<Rc<String>, Pending<Rc<String>>> = HashMap::new();
        // Add explicitly listed containers
        for name in explicit_container_names {
            let key : Rc<String> = name.clone();
            let _ = container_index.insert(key, Pending { explicit: true, todo: name.clone() });
        }

        // Add enumerated containers
        let enum_result;
        if self.config.enumerate {
            debug!("Enumerating containers");
            let mut enumeration = Vec::new();
            if let Err(e) = self.inspector.enumerate(&mut enumeration) {
                debug!(concat!("Enumeration failed. Program will continue but the following ",
                    "error will be returned: {}"), e);
                enum_result = Err(e)
            } else {
                enum_result = Ok(());
                for name in enumeration.drain(..) {
                    let boxed_name = Rc::new(name);
                    let key = boxed_name.clone();
                    container_index.entry(key).or_insert(Pending { explicit: false, todo: boxed_name });
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
                rsig.recv() => {
                    debug!("Received termination signal. Shutting down.");
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
        specs: inspection.todo.specs
    }
}

pub fn run(config: Arc<Config>, inspector: Box<Inspect>, publisher: Box<Publish>,
            termination_signal: chan::Receiver<Signal>, explicit_container_names: &Vec<Rc<String>>)
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
            let current_container = name.todo.clone();
            let was_explicit = name.explicit;

            // Retrieve requested configuration from the container.
            debug!("Inspect {}", current_container);
            let inspection = match ctx.inspect(name) {
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
                    log!(level, "Failed to inspect {}. Skipping. Error: {}", current_container, e);
                    if consider_error {
                        errors.push(e)
                    }
                    continue;
                },
                Ok(x) => x
            };

            // Handle missing env var
            if !inspection.todo.envvar_present {
                let level;
                match (was_explicit, config.missing_envvar) {
                    (true, MissingEnvVarHandling::Automatic) |  (_, MissingEnvVarHandling::Report) => {
                        level = LogLevel::Error;
                        errors.push(CompanionError::EnvVarMissing(
                            current_container.clone(), config.envvar.to_owned()))
                    },
                    (_,_) => {
                        level = LogLevel::Info
                    }
                }
                log!(level, "No environment variable '{}' configured for container {}. Skipping.",
                    config.envvar, current_container);
                continue;
            }

            // Publish updated configuration
            let publication = to_publication(inspection);

            if config.dry_run {
                info!("DRY RUN: would update {} with {:#?}", current_container, publication)
            }
            else {
                info!("Updating configuration for container {}. Publishing {:?}",
                    current_container, publication);
                if let Err(e) = ctx.publish(publication) {
                    error!("Failed to publish updated configuration for container '{}'. Error: {}",
                    current_container, e);
                    errors.push(e);
                }
            }
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
                return Ok(())
            } else {
                return Err(errors)
            }
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
        Pending { explicit: self.explicit, todo: f(self.todo) }
    }
    pub fn try_map<R, E, F: FnOnce(T) -> Result<R, E>>(self, f: F) -> Result<Pending<R>, E> {
        let explicit = self.explicit;
        f(self.todo).map(|t| Pending { explicit: explicit, todo: t })
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

impl<T: Eq> Eq for Pending<T> { }
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