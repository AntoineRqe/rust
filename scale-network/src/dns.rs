use std::cell::RefCell;
use std::rc::Rc;
use crate::mock::{IPAMMock};
use std::net::Ipv4Addr;

pub struct DNS {
    pub ipam: Rc<RefCell<IPAMMock>>,
}

struct FQDN {
    resource_name: String,
    private_network_name: String,
    //internal: String
}

impl FQDN {
    pub fn parse(fqdn: &str) -> Option<Self> {
        let parts: Vec<&str> = fqdn.split('.').collect();
        if parts.len() == 3 && parts[2] == "internal" {
            return Some(Self {
                resource_name: parts[0].to_string(),
                private_network_name: parts[1].to_string(),
                //internal: parts[2].to_string(),
            });
        }
        None
    }
}

impl DNS {
    pub fn new(ipam: Rc<RefCell<IPAMMock>>) -> Self {
        Self { ipam }
    }

    // DNS Records Format: Clients can use DNS to retrieve a resource's IP address in the format: <resource_name>.<private_network_name>.internal.
    pub fn resolve(&self, resource_name: &str) -> Option<Ipv4Addr> {
        let fqdn = FQDN::parse(resource_name)?;
         
        let ipam_borrow = self.ipam.borrow();
        // How do we convert PN id to PN name?
        // I assume VNI and PN name are the same.
        let resources = ipam_borrow.resources.get(&fqdn.private_network_name.parse::<u32>().ok()?)?;

        let resource = resources.get(&fqdn.resource_name)?;
        Some(resource.ip)
    }
}