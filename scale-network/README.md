# Global network layout

Following the description of the network layout provided, here how the network is structured:

```
----------------------- VPC -------------------------           
|                                                   |
|  --------------------     ---------------------   |
|  |        PN A      |     |        PN B       |   |
|  |                  |     |                   |   |
|  |  -------------   |     |  --------------   |   |
|  |  | Resource 1 |  |     |  | Resource 3  |  |   |
|  |  -------------   |     |  --------------   |   |
|  |  | Resource 2 |  |     |  | Resource 4  |  |   |
|  |  -------------   |     |  --------------   |   |
|  |                  |     |                   |   |
|  --------------------     ---------------------   |
|                                                   |
-----------------------------------------------------
```

## DHCP

```rust
pub struct DHCP {
    pub vpc: Rc<RefCell<VPCMock>>,
    pub ipam: Rc<RefCell<IPAMMock>>,
}
```

DHCP owns shared ownership of IPAM and VPC. It is responsible for allocating and releasing IP addresses to resources in the network. It interacts with IPAM to manage the IP address pool and with VPC to ensure that resources are correctly associated with their respective PNs.

## DNS

```rust
pub struct DNS {
    pub ipam: Rc<RefCell<IPAMMock>>,
}

struct FQDN {
    resource_name: String,
    private_network_name: String,
    //internal: String
}
```

DNS also share ownership of IPAM. Its role is to retrieve the IP address associated with a resource name and private network name. It uses the information stored in IPAM to resolve the fully qualified domain name (FQDN) to an IP address.

## Mock

The mock implementation provides a simplified version of the network components for testing purposes. It includes mock versions of IPAM, VPC, and resources, allowing us to test the functionality of DHCP and DNS without needing a full implementation of the network.

### Subnet

```rust
pub struct Subnet {
    pub network: Ipv4Addr, // "192.168.1.0"
    pub mask: Ipv4Addr, // "255.255.255.0"
    pub ips: VecDeque<Ipv4Addr>, // Next available IP address in the subnet
}
```

All available IPs in the subnet are pre-computed and stored in a `VecDeque`. When an IP address is assigned, it is popped from the front of the queue. When an IP address is released, it is pushed back to the end of the queue. This ensures that IP addresses are reused efficiently and in a predictable order.

### VPC (Virtual Private Cloud)

```rust
pub struct PrivateNetwork {
    pub subnets: Vec<Subnet>,   // List of subnets associated with the private network
}
```

```rust
pub struct VPC {
    pub private_networks: HashMap<VniID, PrivateNetwork>, // Mapping of VNI ID to Private Network
}
```

VPC contains a mapping of VNI IDs to their corresponding private networks. Each private network can have multiple subnets, and each subnet manages its own pool of IP addresses.

### IPAM (IP Address Management)

```rust
pub struct ResourceIPInfo {
    pub ip: Ipv4Addr, // IP address assigned to the resource
    pub src: IPSource, // Source of the IP address (either a PN ID or a subnet ID)
    pub resource: Resource, // Resource associated with the IP address
}

pub struct IPAMMock {
    pub resources: HashMap<VniID, HashMap<String, ResourceIPInfo>>,
}
```

IPAMMock keep track of the IP addresses assigned to resources in each VNI.

## Tests

All tests are located in the `tests` directory. To run the tests, use the following command:

```bash
cargo test
```