# Structure of generated market feed

Market data is generated in binary format. Each message contains a header and a body.

## Header

- `message_type` (1 byte): Indicates the type of the message (e.g., trade, quote, etc.).
- `version` (1 byte): The version of the message format.
- `seq_num` (8 bytes): A sequence number that increments with each message, used to keep track of the order of messages.
- `timestamp_ns` (8 bytes): The timestamp of the message in nanoseconds since the Unix epoch.
- `instrument_id` (4 bytes): An identifier for the financial instrument
- `length` (2 bytes): The total length of the message, including the header and body.


## Body

The body of the message contains the specific data relevant to the message type. The message types implemented are:

- AddOrder: Represents the addition of a new order to the order book.
- ModifyOrder: Represents the modification of an existing order in the order book.
- DeleteOrder: Represents the deletion of an existing order from the order book.
- Trade: Represents a trade that has occurred between two orders.
- Snapshot: Represents a snapshot of the current state of the order book for a given instrument.    

### AddOrder

