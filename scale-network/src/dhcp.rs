use crate::mock::{VPCMock, IPAMMock};
use std::net::{Ipv4Addr};
use crate::mock::Resource;
use std::rc::Rc;
use std::cell::RefCell;

// Use Rc and RefCell to allow shared ownership and interior mutability of the VPC and IPAM mocks
// VPC and IPAM can be shared between DHCP and DNS, and can be mutated by both
// Improvement: Add synchronization primitives (e.g., RwLock, Mutex) for handle concurrency.
pub struct DHCP {
    pub vpc: Rc<RefCell<VPCMock>>,
    pub ipam: Rc<RefCell<IPAMMock>>,
}

impl DHCP {
    pub fn new(vpc: Rc<RefCell<VPCMock>>, ipam: Rc<RefCell<IPAMMock>>) -> Self {
        Self { vpc, ipam }
    }

    /// Assign an IP address to a resource in the given VNI
    pub fn assign_ip(&mut self, vni: u32, resource: Resource) -> Result<Ipv4Addr, String> 
    {
        // Try to retrieve an IP address from the VPC mock for the given VNI
        match self.vpc.borrow_mut().get_ip_by_vni(vni) {
            Some(ip) => {
                // Assign the IP address to the resource in the IPAM mock
                self.ipam.borrow_mut().add_resource(vni, ip, resource)?;
                return Ok(ip);
            },
            None => Err(format!("No available IP addresses in PN with VNI {}", vni)),
        }
    }

    pub fn release_resource(&mut self, vni: u32, resource: &str) -> Result<(), String> {
        // Release the IP address from the resource in the IPAM mock
        let resource = self.ipam.borrow_mut().release_resource(vni, resource)?;
        // Release the IP address back to the VPC mock
        self.vpc.borrow_mut().release_ip(vni, resource.ip)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{
        Resource,
        IPAMMock,
    };

    #[test]
    fn test_dhcp_assign_and_release() {
        let vpc = Rc::new(RefCell::new(VPCMock::create_mock()));
        let ipam = Rc::new(RefCell::new(IPAMMock::new()));

        let mut dhcp = DHCP::new(vpc.clone(), ipam.clone());

        // Assign an IP address to a resource in VNI 1
        let resource = Resource::new(1, "TestResource".to_string(), "00:11:22:33:44:55".to_string());
        let assigned_ip = dhcp.assign_ip(1, resource.clone()).expect("Failed to assign IP");
        println!("Assigned IP: {}", assigned_ip);

        // Verify that the IP address is assigned in the IPAM mock
        {
            let ipam_borrow = ipam.borrow();
            let ip_info = ipam_borrow.resources.get(&1).and_then(|ip_map| ip_map.iter().find(|ipm| ipm.1.ip == assigned_ip));
            assert!(ip_info.is_some(), "IP address should be assigned in IPAM mock");
            assert_eq!(ip_info.unwrap().1.resource.id, resource.id, "Resource ID should match in IPAM mock");
        }

        // Release the IP address
        dhcp.release_resource(1, &resource.name).expect("Failed to release IP");

        // Verify that the IP address is released in the IPAM mock
        {
            let ipam_borrow = ipam.borrow();
            let ip_info = ipam_borrow.resources.get(&1).and_then(|ip_map| ip_map.iter().find(|ipm| ipm.1.ip == assigned_ip));
            assert!(ip_info.is_none(), "IP address should be released in IPAM mock");
        }

    }
}