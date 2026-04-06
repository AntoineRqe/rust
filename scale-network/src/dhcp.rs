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
    /// Returns the assigned IP address or an error message if assignment fails
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

    /// Release an IP address associated with a resource in the given VNI
    /// Returns Ok(()) if the release is successful or an error message if it fails
    pub fn release_resource(&mut self, vni: u32, resource: &str) -> Result<Ipv4Addr, String> {
        // Release the IP address from the resource in the IPAM mock
        let resource = self.ipam.borrow_mut().release_resource(vni, resource)?;
        // Release the IP address back to the VPC mock
        self.vpc.borrow_mut().release_ip(vni, resource.ip)?;
        Ok(resource.ip)
    }
}
