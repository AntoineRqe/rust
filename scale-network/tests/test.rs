use scale_network::dhcp::DHCP;
use scale_network::dns::DNS;
use scale_network::mock::{IPAMMock, VPCMock, Resource, VniID, ResourceID};
use std::rc::Rc;
use std::cell::RefCell;
use std::net::Ipv4Addr;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dhcp_assign_and_release() {
        let vpc = Rc::new(RefCell::new(VPCMock::create_mock()));
        let ipam = Rc::new(RefCell::new(IPAMMock::new()));

        let mut dhcp = DHCP::new(vpc.clone(), ipam.clone());

        let vni_id : VniID = 1;
        let resource_id : ResourceID = 1;
        let resource = Resource::new(resource_id, "TestResource".to_string(), "00:11:22:33:44:55".to_string());

        // Assign an IP address to a resource in VNI 1
        let assigned_ip = dhcp.assign_ip(vni_id, resource.clone()).expect("Failed to assign IP");

        // Verify that the IP address is assigned in the IPAM mock
        {
            let ipam_borrow = ipam.borrow();
            let resources = ipam_borrow.resources.get(&vni_id).expect("PN with VNI 1 should exist in IPAM mock");
            assert!(resources.iter().any(|(_, ipm)| ipm.ip == assigned_ip && ipm.resource.id == resource.id), "Resource should be assigned in IPAM mock");
        }

        // Release the IP address
        let release_ip = dhcp.release_resource(vni_id, &resource.name).expect("Failed to release IP");
        assert!(release_ip == assigned_ip, "Released IP should match the assigned IP");
    }

    #[test]
    fn test_dhcp_assign_ip_no_available_ips() {
        let vpc = Rc::new(RefCell::new(VPCMock::create_mock()));
        let ipam = Rc::new(RefCell::new(IPAMMock::new()));

        let mut dhcp = DHCP::new(vpc.clone(), ipam.clone());

        let vni_id : VniID = 2;
        let resource_id : ResourceID = 1;
        let mut resource = Resource::new(resource_id, "TestResource".to_string(), "00:11:22:33:44:55".to_string());

        // Exhaust all available IP addresses in VNI 2 (Only 254 IPs available in a /24 subnet)
        for i in 0..254 {
            resource.id = i + 1; // Update resource ID for each assignment
            resource.name = format!("TestResource{}", i + 1); // Update resource name for each assignment
            let res = dhcp.assign_ip(vni_id, resource.clone());
            assert!(res.is_ok(), "Failed to assign IP iteration {} : {:?}", i, res.err().unwrap());
        }

        resource.id = 255;
        resource.name = "TestResource255".to_string();
        assert!(dhcp.assign_ip(vni_id, resource.clone()).is_err(), "Assigning IP should fail when no available IPs in VNI");
    }

    #[test]
    fn test_dns_resolve() {
        let ipam = Rc::new(RefCell::new(IPAMMock::new()));
        let dns = DNS::new(ipam.clone());
        let vni: VniID = 1;

        // Add a resource to the IPAM mock
        let resource = Resource::new(1, "TestResource".to_string(), "00:11:22:33:44:55".to_string());
        ipam.borrow_mut().add_resource(vni, Ipv4Addr::new(192, 168, 1, 10), resource.clone()).expect("Failed to add resource to IPAM mock");
        // Resolve the resource's IP address using DNS
        let fqdn = format!("{}.{}.internal", resource.name, vni);
        let resolved_ip = dns.resolve(&fqdn).expect("Failed to resolve resource using DNS");
        assert_eq!(resolved_ip, Ipv4Addr::new(192, 168, 1, 10), "Resolved IP address should match the assigned IP");
    }

    #[test]
    fn test_dns_resolve_nonexistent_resource() {
        let ipam = Rc::new(RefCell::new(IPAMMock::new()));
        let dns = DNS::new(ipam.clone());
        let vni: VniID = 1;

        // Attempt to resolve a non-existent resource
        let fqdn = format!("{}.{}.internal", "NonExistentResource", vni);
        let resolved_ip = dns.resolve(&fqdn);
        assert!(resolved_ip.is_none(), "Resolving a non-existent resource should return None");
    }

    #[test]
    fn test_dns_resolve_invalid_fqdn() {
        let ipam = Rc::new(RefCell::new(IPAMMock::new()));
        let dns = DNS::new(ipam.clone());
        // Attempt to resolve an invalid FQDN
        let fqdn = "invalid.fqdn";
        let resolved_ip = dns.resolve(fqdn);
        assert!(resolved_ip.is_none(), "Resolving an invalid FQDN should return None");
    }

    #[test]
    fn test_integration_dhcp_and_dns() {
        let vpc = Rc::new(RefCell::new(VPCMock::create_mock()));
        let ipam = Rc::new(RefCell::new(IPAMMock::new()));

        let mut dhcp = DHCP::new(vpc.clone(), ipam.clone());
        let dns = DNS::new(ipam.clone());

        let vni_id : VniID = 1;
        let resource_id : ResourceID = 1;
        let resource = Resource::new(resource_id, "TestResource".to_string(), "00:11:22:33:44:55".to_string());

        // Assign an IP address to a resource in VNI 1
        let assigned_ip = dhcp.assign_ip(vni_id, resource.clone()).expect("Failed to assign IP");

        // Resolve the resource's IP address using DNS
        let fqdn = format!("{}.{}.internal", resource.name, vni_id);
        let resolved_ip = dns.resolve(&fqdn).expect("Failed to resolve resource using DNS");
        assert_eq!(resolved_ip, assigned_ip, "Resolved IP address should match the assigned IP");

        // Release the IP address
        dhcp.release_resource(vni_id, &resource.name).expect("Failed to release IP");

        // Attempt to resolve the resource's IP address again after release
        let resolved_ip_after_release = dns.resolve(&fqdn);
        assert!(resolved_ip_after_release.is_none(), "Resolving a released resource should return None");
    }
}