use std::net::Ipv4Addr;
use std::collections::HashMap;
use std::collections::VecDeque;

// -----------------------------------------------------------------
// ---------------------------- Subnet -----------------------------
// -----------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Subnet {
    pub network: Ipv4Addr, // "192.168.1.0"
    pub mask: Ipv4Addr, // "255.255.255.0"
    pub ips: VecDeque<Ipv4Addr>, // Next available IP address in the subnet
}

impl Subnet {
    fn compute_available_ips(network: Ipv4Addr, mask: Ipv4Addr) -> VecDeque<Ipv4Addr> {
    
        let network_u32 = u32::from(network);
        let mask_u32 = u32::from(mask);

        let network_address = network_u32 & mask_u32;
        let broadcast_address = network_address | !mask_u32;

        let len = broadcast_address - network_address - 1;
        let mut ips = VecDeque::with_capacity(len as usize);

        for ip_u32 in (network_address + 1)..broadcast_address {
            ips.push_back(Ipv4Addr::from(ip_u32));
        }
        ips
    }

    pub fn new(network: Ipv4Addr, mask: Ipv4Addr) -> Self {
        let ips = Self::compute_available_ips(network, mask);
        let subnet = Self { network, mask, ips };
        subnet
    }

    pub fn get_ip(&mut self) -> Option<Ipv4Addr> {
        self.ips.pop_front()
    }

    pub fn add_ip(&mut self, ip: Ipv4Addr) {
        self.ips.push_back(ip);
    }
}

#[test]
fn test_subnet() {
    let mut subnet = Subnet::new(Ipv4Addr::new(192, 168, 1, 0), Ipv4Addr::new(255, 255, 255, 0));
    for _ in 0..254 {
        let ip = subnet.get_ip();
        assert!(ip.is_some());
    }
    assert_eq!(subnet.get_ip(), None);

    subnet.add_ip(Ipv4Addr::new(192, 168, 1, 1));
    assert_eq!(subnet.get_ip(), Some(Ipv4Addr::new(192, 168, 1, 1)));
}

// -----------------------------------------------------------------
// ------------------- Private Network (PN) ------------------------
// -----------------------------------------------------------------

pub struct PrivateNetwork {
    pub subnets: Vec<Subnet>,   // List of subnets associated with the private network
}

impl PrivateNetwork {
    pub fn new(subnets: Vec<Subnet>) -> Self {
        Self { subnets }
    }
}

// -----------------------------------------------------------------
// ------------------- VPC (Virtual Private Cloud) -----------------
// -----------------------------------------------------------------

pub type VniID = u32; // VNI (Virtual Network Identifier)

/// Virtual Private Cloud (VPC)
pub struct VPCMock {
    // Key is the VNI (Virtual Network Identifier)
    // Value is the private network associated with the VNI
    pub private_networks: HashMap<VniID, PrivateNetwork>,    
}

// VPC API contains information about PNs and their subnets.
// Each PN has a VNI and a list of subnets associated.
// A VPC also has a routing table associated but is out-of-scope for your project.
impl VPCMock {
    pub fn new() -> Self {
        Self {
            private_networks: HashMap::new(),
        }
    }

    pub fn get_ip_by_vni(&mut self, vni: VniID) -> Option<Ipv4Addr> {
        let pn = self.private_networks.get_mut(&vni);

        if let Some(pn) = pn {
            // Optimization: Time complexity is O(n) where n is the number of subnets in the PN.
            // Can I own in advance which subnet should be used for each VNI to avoid iterating over all subnets?
            for subnet in &mut pn.subnets {
                if let Some(ip) = subnet.get_ip() {
                    return Some(ip);
                }
            }
        }
        None
    }


    pub fn release_ip(&mut self, vni: VniID, ip: Ipv4Addr) -> Result<(), String> {
        let pn = self.private_networks.get_mut(&vni);

        // Optimization: Time complexity is O(n) where n is the number of subnets in the PN.
        // Mapping between IP address and VPC mock ?
        if let Some(pn) = pn {
            for subnet in &mut pn.subnets {
                let network_u32 = u32::from(subnet.network);
                let mask_u32 = u32::from(subnet.mask);
                let ip_u32 = u32::from(ip);

                // Check if the IP address belongs to the subnet
                if (ip_u32 & mask_u32) == (network_u32 & mask_u32) {
                    subnet.add_ip(ip);
                    return Ok(());
                }
            }
            Err(format!("IP address {} does not belong to any subnet in PN with VNI {}", ip, vni))
        } else {
            Err(format!("PN with VNI {} not found", vni))
        }
    }

    // Create a mock VPC with predefined PNs and subnets for testing purposes
    pub fn create_mock() -> Self {
        let mut private_networks = HashMap::new();

        private_networks.insert(
            1,
            PrivateNetwork {
                subnets: vec![
                    Subnet::new(Ipv4Addr::new(192, 168, 1, 0), Ipv4Addr::new(255, 255, 255, 0)),
                    Subnet::new(Ipv4Addr::new(192, 168, 2, 0), Ipv4Addr::new(255, 255, 255, 0)),
                ],
            },
        );

        private_networks.insert(
            2,
            PrivateNetwork {
                subnets: vec![
                    Subnet::new(Ipv4Addr::new(10, 0, 0, 0), Ipv4Addr::new(255, 255, 255, 0)),
                ],
            },
        );

        VPCMock { private_networks }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vpc_get_and_release_ip() {
        let mut vpc = VPCMock::new();

        vpc.private_networks.insert(
            1,
            PrivateNetwork {
                subnets: vec![
                    Subnet::new(Ipv4Addr::new(192, 168, 1, 0), Ipv4Addr::new(255, 255, 255, 0)),
                ],
            },
        );

        // Get an IP address from the VPC mock
        let ip = vpc.get_ip_by_vni(1);
        assert!(ip.is_some(), "Should get an IP address from the VPC mock");
        let ip = ip.unwrap();
        assert_eq!(ip, Ipv4Addr::new(192, 168, 1, 1));
    }
}



// -----------------------------------------------------------------
// ---------------------------- IPAM -----------------------------
// -----------------------------------------------------------------

pub type ResourceID = u32; // Resource ID

#[derive(Debug, Clone)]
pub struct Resource {
    pub id: ResourceID,
    pub name: String,
    // TODO: Add MAC address as [u8] to avoid allocation at runtime
    pub mac_address: String,
}

impl Resource {
    pub fn new(id: ResourceID, name: String, mac_address: String) -> Self {
        Self { id, name, mac_address }
    }
}

pub enum IPSource {
    PN(VniID), // Source is a PN ID
    Subnet(Ipv4Addr), // Source is a subnet ID (network address)
}

pub struct ResourceIPInfo {
    pub ip: Ipv4Addr, // IP address assigned to the resource
    pub src: IPSource, // Source of the IP address (either a PN ID or a subnet ID)
    pub resource: Resource, // Resource associated with the IP address
}


// IPAM API contains information about IP addresses used by resources in a PN or subnet.
// Each IP address has a source (either a PN ID or a subnet ID) and a resource associated.
// The resource includes its ID, name and MAC address.
pub struct IPAMMock {
    // Key is the VNI
    // Value is a map of resource name to ResourceIPInfo
    pub resources: HashMap<VniID, HashMap<String, ResourceIPInfo>>,
}

impl IPAMMock {
    pub fn new() -> Self {
        Self {
            resources: HashMap::new(),
        }
    }

    pub fn add_resource(&mut self, vni: VniID, ip: Ipv4Addr, resource: Resource) -> Result<(), String> {
        let resource = ResourceIPInfo { src: IPSource::PN(vni), ip, resource };
        let resources_map = self.resources.entry(vni).or_insert_with(HashMap::new);
        
        if resources_map.contains_key(&resource.resource.name) {
            Err(format!("Resource {} is already assigned in PN with VNI {}", resource.resource.name, vni))
        } else {
            resources_map.insert(resource.resource.name.clone(), resource);
            Ok(())
        }
    }

    pub fn release_resource(&mut self, vni: VniID, resource: &str) -> Result<ResourceIPInfo, String> {
        let resources_map = self.resources.get_mut(&vni);

        if let Some(resources_map) = resources_map {
            if resources_map.contains_key(resource) {
                Ok(resources_map.remove(resource).unwrap())
            } else {
                Err(format!("Resource {} is not assigned in PN with VNI {}", resource, vni))
            }
        } else {
            Err(format!("PN with VNI {} not found", vni))
        }
    }
}


#[test]
fn test_ipam_add_and_release_resource() {
    let mut ipam = IPAMMock::new();
    let resource = Resource::new(1, "TestResource".to_string(), "00:11:22:33:44:55".to_string());
    let ip = Ipv4Addr::new(192, 168, 1, 1);

    // Add a resource to the IPAM mock
    ipam.add_resource(1, ip, resource.clone()).expect("Failed to add resource to IPAM mock");

    assert!(ipam.resources.get(&1).is_some(), "PN with VNI 1 should exist in IPAM mock");
    let ip_map = ipam.resources.get(&1).unwrap();
    assert!(ip_map.iter().any(|(_, ipm)| ipm.ip == ip && ipm.resource.id == resource.id), "Resource should be added to IPAM mock");

    // Release the resource from the IPAM mock
    let released_resource = ipam.release_resource(1, &resource.name).expect("Failed to release resource from IPAM mock");
    assert_eq!(released_resource.ip, ip, "Released resource should have the correct IP address");
    assert_eq!(released_resource.resource.id, resource.id, "Released resource should have the correct ID");
}