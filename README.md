# Swarm Mesh

A decentralized peer-to-peer mesh networking system built in Rust that enables secure communication between nodes using the Noise protocol for cryptographic security.

## Overview

Swarm Mesh is a distributed networking system where nodes automatically discover each other via multicast broadcasts and establish secure, authenticated connections. The system uses the Noise protocol (specifically `Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s`) for end-to-end encryption and authentication.

## Architecture

The project is organized into several crates:

- **`apps/node`**: Main application binary that runs a mesh node
- **`crates/mesh`**: Core mesh networking types and message definitions
- **`crates/node`**: Node implementation with connection management and state handling
- **`crates/noise`**: Noise protocol wrapper for secure handshakes

## Key Features

### 1. Automatic Peer Discovery
- Nodes broadcast `Hello` messages on multicast (`224.0.0.1:9999`)
- Peers automatically discover each other without central coordination
- Each node has a unique 32-bit identifier

### 2. Secure Communication
- Uses Noise protocol for authenticated encryption
- Pre-shared key (PSK) for network-level authentication
- Elliptic curve cryptography (Curve25519) for key exchange
- ChaCha20-Poly1305 for symmetric encryption

### 3. Connection Management
- Automatic handshake initiation based on node ID comparison
- Session state management for each peer
- Graceful handling of connection failures

### 4. Network Monitoring
- RTT (Round Trip Time) measurement via ping/pong messages
- Connection health monitoring
- Automatic cleanup of stale connections

## Network Protocol

### Message Types

1. **Hello**: Multicast discovery messages
2. **Handshake**: Noise protocol handshake messages
3. **Ping/Pong**: Keep-alive and RTT measurement
4. **EncryptedData**: Application data (encrypted)

### Connection Flow

1. **Discovery**: Nodes broadcast Hello messages every 15 seconds
2. **Handshake Initiation**: Higher node ID initiates handshake
3. **Secure Session**: Established after successful Noise handshake
4. **Monitoring**: Regular ping/pong messages every 8 seconds

## Configuration

### Network Settings
- **Multicast Address**: `224.0.0.1:9999`
- **Network Key**: 32-byte pre-shared key for authentication
- **Hello Interval**: 15 seconds
- **Ping Interval**: 8 seconds
- **Ping Timeout**: 30 seconds

## Running the Application

```bash
# Build the project
cargo build --release

# Run a node
cargo run --bin node-svc

# Run with debug logging
RUST_LOG=debug cargo run --bin node-svc
```

## Logging

The application uses structured logging with tracing:
- JSON format for machine-readable logs
- Configurable via `RUST_LOG` environment variable
- Detailed connection and protocol event logging

## Security Considerations

- All inter-node communication is encrypted after handshake
- Network access requires knowledge of the pre-shared key
- Each node generates unique ephemeral keys
- Forward secrecy through Noise protocol design

## Dependencies

- **tokio**: Async runtime
- **snow**: Noise protocol implementation
- **socket2**: Advanced socket operations
- **bincode**: Binary serialization
- **tracing**: Structured logging
- **anyhow**: Error handling 