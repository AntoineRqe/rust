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

### Design choices

- **IP Address Management**: The decision to pre-compute all available IP addresses in a subnet and store them in a `VecDeque` allows for efficient allocation and release of IP addresses. This approach ensures that IP addresses are reused in a predictable order. Maybe IPAM should handle the IP address pool instead of the VPC mock to centralize IP management and avoid potential inconsistencies between the VPC and IPAM mocks. In my implementation, DHCP is responsible for the synchronization between the VPC and IPAM mocks, but this could lead to issues if not handled carefully.

- **Resource Management**: I decided to use a `HashMap` to store resources in the IPAM mock, with the resource name as the key. This allows for efficient lookups when allocating and releasing resources. Based on the resource name, we can easily retrieve the associated IP address and other information.

- **Multi threading**: The use of `Rc<RefCell<T>>` for shared ownership of the VPC and IPAM mocks allows for mutable access to these components across different parts of the code. However, this approach is not thread-safe and may lead to issues in a multi-threaded environment. If we need to support multi-threading in the future, we may need to consider using `Arc<Mutex<T>>` instead to ensure thread safety. But `Mutex` is not ideal for performance, so we may need to consider other synchronization mechanisms or design patterns to manage concurrent access to shared resources effectively.