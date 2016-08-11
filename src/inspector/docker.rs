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

use std::sync::Arc;
use std::fmt::{self, Display};
use std::error::Error;
use std::convert::From;

use shiplift::{self, Docker};
use shiplift::builder::{ContainerListOptions,ContainerFilter};

use common::Config;
use domain_spec::{self, DomainSpec};
use super::*;

pub struct DockerInspector {
    config: Arc<Config>,
    docker_client_opt: Option<Docker>,
}

impl DockerInspector {
    pub fn new(config: Arc<Config>) -> DockerInspector {
        DockerInspector { config: config, docker_client_opt: None }
    }

    fn create_docker_client(&mut self) -> &mut Docker {
        if let Some(ref mut client) = self.docker_client_opt {
            client
        } else {
            let client = Docker::host(self.config.docker_url.clone());
            self.docker_client_opt = Some(client);
            self.docker_client_opt.as_mut().unwrap()
        }
    }
}

impl Inspect for DockerInspector {
    fn enumerate(&mut self, container_names: &mut Vec<String>) -> Result<(), InspectionError> {
        let docker = self.create_docker_client();
        let containers_api = docker.containers();
        let list_options = ContainerListOptions::builder()
            .filter(vec![ContainerFilter::Status("running".to_owned())])
            .build();
        let containers = try!(containers_api.list(&list_options));
        debug!("Found {} running containers.", containers.len());
        for container in containers {
            container_names.push(container.Names.first().unwrap_or(&container.Id).to_owned())
        }
        Ok(())
    }

    fn inspect(&mut self, container_name: &str) -> Result<Inspection, InspectionError> {
        let config: Arc<Config> = self.config.clone();
        let (container_host, env_opt) = {
            let docker = self.create_docker_client();
            let containers = docker.containers();
            let container_ref = containers.get(container_name);
            let container = try!(container_ref.inspect());
            // When docker network is active, we use the container name (=hostname)
            // otherwise, we use "IPAddress", which hopefully maps to the ip on the bridge
            // interface. At this point, the shiplift library doesn't know about 'docker networks'
            let container_host = if config.docker_network {
                container_name.to_owned()
            } else {
                container.NetworkSettings.IPAddress
            };


            let env_opt = container.Config.Env;
            (container_host, env_opt)
        };

        let mut envvar_present = false;
        let mut specs = Vec::new();
        try!(parse_container_env_vars(&env_opt, &config, &mut envvar_present, &mut specs));
        Ok(Inspection { envvar_present: envvar_present, specs: specs, host: container_host })
    }
}

fn parse_container_env_vars(env_opt: &Option<Vec<String>>,
                            config: &Config,
                            envvar_present: &mut bool,
                            specs: &mut Vec<DomainSpec>)
                            -> Result<(), InspectionError> {
    if let Some(ref env) = *env_opt {
        for line in env.iter() {
            let parts: Vec<&str> = line.splitn(2, '=').collect();
            if parts.len() < 2 || &parts[0] != &*config.envvar {
                continue;
            }
            *envvar_present = true;
            try!(DomainSpec::parse_all(&parts[1], specs));
        }
    }
    Ok(())
}

// ############### INSPECTION ERROR #######################

impl InspectionInnerError for domain_spec::DomainSpecError {}


impl From<Box<domain_spec::DomainSpecError>> for InspectionError {
    fn from(val: Box<domain_spec::DomainSpecError>) -> InspectionError {
        InspectionError { inner: Box::new(val) }
    }
}

#[derive(Debug)]
struct ShipliftError {
    actual: shiplift::errors::Error,
}

impl Error for ShipliftError {
    fn cause(&self) -> Option<&Error> {
        None
    }
    fn description(&self) -> &'static str {
        "Error while communicating with the docker daemon."
    }
}

impl Display for ShipliftError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "{} Details: {:?}", self.description(), self.actual)
    }
}

impl From<Box<ShipliftError>> for InspectionError {
    fn from(val: Box<ShipliftError>) -> InspectionError {
        InspectionError { inner: Box::new(*val) }
    }
}

impl InspectionInnerError for ShipliftError {}

impl From<shiplift::errors::Error> for InspectionError {
    fn from(err: shiplift::errors::Error) -> InspectionError {
        From::from(ShipliftError { actual: err })
    }
}

// ############### TESTING ################################

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use common::{self, Config};
    use super::*;
    use super::parse_container_env_vars;


    #[test]
    fn no_env_vars() {
        common::init_log();
        // #### GIVEN ####
        let mut specs = Vec::new();
        let mut present = false;
        let config: Config = Default::default();
        let env = None;

        // #### WHEN  ####
        parse_container_env_vars(&env, &config, &mut present, &mut specs)
            .expect("parse_container_env_vars shouldn't fail");

        // #### THEN  ####
        assert!(!present, "envvar was present");
        assert_eq!(specs.len(), 0);
    }

    #[test]
    fn var_not_present() {
        common::init_log();
        // #### GIVEN ####
        let mut specs = Vec::new();
        let mut present = false;
        let config: Config = Default::default();
        let env = Some(vec!["PATH=abc".to_owned(),
                            "imanenvvariswear".to_owned(),
                            "NOT_BEACHHEAD=example.org".to_owned()]);

        // #### WHEN  ####
        parse_container_env_vars(&env, &config, &mut present, &mut specs)
            .expect("parse_container_env_vars shouldn't fail");

        // #### THEN  ####
        assert!(!present, "envvar was present");
        assert_eq!(specs.len(), 0);
    }

    #[test]
    fn missing_value() {
        common::init_log();
        // #### GIVEN ####
        let mut specs = Vec::new();
        let mut present = false;
        let config: Config = Default::default();
        let env = Some(vec![
            "PATH=abc".to_owned(),
            format!("{}",config.envvar),
            "imanenvvariswear".to_owned(),
        ]);

        // #### WHEN  ####
        parse_container_env_vars(&env, &config, &mut present, &mut specs)
            .expect("parse_container_env_vars shouldn't fail");

        // #### THEN  ####
        assert!(!present, "envvar was present");
        assert_eq!(specs.len(), 0);
    }

    #[test]
    fn single_domain() {
        common::init_log();
        // #### GIVEN ####
        let mut specs = Vec::new();
        let mut present = false;
        let config: Config = Default::default();
        let env = Some(vec![
            "PATH=abc".to_owned(),
            format!("{}=example.org",config.envvar),
            "imanenvvariswear".to_owned(),
        ]);

        // #### WHEN  ####
        parse_container_env_vars(&env, &config, &mut present, &mut specs)
            .expect("parse_container_env_vars shouldn't fail");

        // #### THEN  ####
        assert!(present, "envvar was not present");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].domain_name, "example.org");
    }

    #[test]
    fn multiple_domains_one_var() {
        common::init_log();
        // #### GIVEN ####
        let mut specs = Vec::new();
        let mut present = false;
        let config: Config = Default::default();
        let env = Some(vec![
            "PATH=abc".to_owned(),
            format!("{}=example.org www.example.org",config.envvar),
            "imanenvvariswear".to_owned(),
        ]);

        // #### WHEN  ####
        parse_container_env_vars(&env, &config, &mut present, &mut specs)
            .expect("parse_container_env_vars shouldn't fail");

        // #### THEN  ####
        assert!(present, "envvar was not present");
        assert_eq!(specs.len(), 2);
        // exact parsing is covered in domain_spec tests. We just do some very basic sanity checks.
        assert!(specs[0].domain_name != specs[1].domain_name);
    }

    #[test]
    fn multiple_domains_multiple_vars() {
        common::init_log();
        // #### GIVEN ####
        let mut specs = Vec::new();
        let mut present = false;
        let config: Config = Default::default();
        let env = Some(vec!["PATH=abc".to_owned(),
                            format!("{}=example.org www.example.org", config.envvar),
                            "imanenvvariswear".to_owned(),
                            format!("{}=admin.example.org", config.envvar),
                            "mixed_CaseIs_cool=NOT!!".to_owned()]);

        // #### WHEN  ####
        parse_container_env_vars(&env, &config, &mut present, &mut specs)
            .expect("parse_container_env_vars shouldn't fail");

        // #### THEN  ####
        assert!(present, "envvar was not present");
        assert_eq!(specs.len(), 3);
        // exact parsing is covered in domain_spec tests. We just do some very basic sanity checks.
        assert!(specs[0].domain_name != specs[1].domain_name);
        assert!(specs[0].domain_name != specs[2].domain_name);
        assert!(specs[2].domain_name != specs[1].domain_name);
    }

    #[test]
    fn one_valid_one_invalid() {
        common::init_log();
        // #### GIVEN ####
        let mut specs = Vec::new();
        let mut present = false;
        let config: Config = Default::default();
        let env = Some(vec![
            format!("{}=example.org", config.envvar),
            format!("{}=!!*&^$#$:", config.envvar),
        ]);

        // #### WHEN  ####
        parse_container_env_vars(&env, &config, &mut present, &mut specs)
            .expect("parse_container_env_vars shouldn't fail");

        // #### THEN  ####
        assert!(present, "envvar was not present");
        assert_eq!(specs.len(), 1);
        // exact parsing is covered in domain_spec tests. We just do some very basic sanity checks.
        assert_eq!(specs[0].domain_name, "example.org");
    }

    #[test]
    fn initialize() {
        common::init_log();
        // #### GIVEN ####
        let config: Arc<Config> = Arc::new(Default::default());

        // #### WHEN  ####
        DockerInspector::new(config.clone());

        // #### THEN  ####
        // no panic
    }
}
