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

DHCP owns shared ownership of IPAM and VPC. It is responsible for allocating and releasing IP addresses to resources in the network. It interacts with IPAM to manage the IP address pool and with VPC to ensure that resources are correctly associated with their respective PNs.

## DNS

DNS also share ownership of IPAM. It is responsible for managing the domain name system for the resources in the network. It interacts with IPAM to ensure that resources have the correct IP addresses associated with their domain names.