# Swarm Mesh

A decentralized peer-to-peer mesh networking system built in Rust that enables secure communication between nodes using
the Noise protocol for cryptographic security.

## Overview

Swarm Mesh is a distributed networking system where nodes automatically discover each other via multicast broadcasts
and establish secure, authenticated connections. The system uses the Noise protocol, specifically,
`Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s` for end-to-end encryption and authentication.

## Architecture

The project is organized into several crates:

- **`apps/node`**: Main application binary that runs a mesh node
- **`apps/mesh-registry`**: Registry service for node discovery and Grafana visualization
- **`crates/mesh`**: Core mesh networking types and message definitions
- **`crates/node`**: Node implementation with connection management and state handling
- **`crates/noise`**: Noise protocol wrapper for secure handshakes
- **`crates/registry`**: Node registry with TTL-based cleanup for service discovery

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

### 5. Metrics and Observability
- Prometheus metrics server for real-time monitoring
- RTT statistics (current, min, max, average) per peer connection
- Connected peers count per node
- Grafana dashboards for network visualization
- Interactive node graph showing mesh topology

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

### Basic Usage

```bash
# Build the project
cargo build --release

# Run a node (port is automatically assigned)
cargo run --bin node

# Run additional nodes
cargo run --bin node &
cargo run --bin node &
cargo run --bin node &

# Run with debug logging
RUST_LOG=debug cargo run --bin node
```

### Command Line Options

- `--mesh-registry-endpoint <URL>`: Mesh registry service endpoint (default: http://localhost:5000)

Each node automatically assigns an ephemeral port for metrics and registers with the mesh-registry service for monitoring discovery.

## Metrics and Monitoring

Swarm Mesh provides comprehensive monitoring capabilities through Prometheus metrics and Grafana dashboards.

### Metrics Collection

Each node runs a built-in HTTP server that exposes Prometheus-compatible metrics:

- **RTT Metrics**: Current, minimum, maximum, and average round-trip times between peers
- **Connection Metrics**: Number of active peer connections per node
- **Network Health**: Overall mesh connectivity and performance statistics

### Available Metrics

- `mesh_peer_rtt_current_milliseconds`: Current RTT to each peer
- `mesh_peer_rtt_min_milliseconds`: Minimum RTT observed to each peer
- `mesh_peer_rtt_max_milliseconds`: Maximum RTT observed to each peer  
- `mesh_peer_rtt_avg_milliseconds`: Average RTT to each peer
- `mesh_connected_peers_total`: Total number of connected peers per node

All RTT metrics include labels: `local_node_id`, `remote_node_id`, `local_ip`, `remote_ip`
Connection metrics include labels: `node_id`, `node_ip`

### Local Monitoring Setup

To monitor your mesh network locally using Docker:

```bash
# Navigate to the docker directory
cd docker

# Start Prometheus and Grafana
docker compose up -d

# Access the services
# Grafana: http://localhost:3000 (admin/admin)
# Prometheus: http://localhost:9090
```

The setup includes:
- **Prometheus**: Metrics collection and storage (port 9090)
- **Grafana**: Visualization and dashboards (port 3000)
- **Pre-configured Dashboard**: "Swarm Mesh Network Dashboard" with:
  - Real-time RTT graphs
  - Connected peers statistics
  - Interactive node graph showing mesh topology
  - Network health summary

### Dynamic Node Discovery

The monitoring setup uses **automatic service discovery** via the mesh-registry service. Start it before running the monitoring stack:

```bash
# Start mesh-registry for dynamic node discovery
cargo run --bin mesh-registry

# Start monitoring stack
cd docker && docker-compose up -d
```
Prometheus automatically discovers active nodes by querying the mesh-registry service. New nodes are automatically added to monitoring when they join the mesh.

#### Mesh Registry Service

The mesh-registry service (`apps/mesh-registry`) provides:
- **Node Registration**: Nodes automatically register their metrics endpoints
- **Service Discovery**: Prometheus target discovery via HTTP API
- **Grafana Integration**: Real-time network graph visualization
- **TTL Management**: Automatic cleanup of stale node registrations

The service runs on port 5000 by default and provides these endpoints:
- `GET /prometheus/targets` - Prometheus service discovery
- `POST /register` - Node registration
- `POST /heartbeat` - Node heartbeat/refresh
- `GET /graph` - Graph data for Grafana node graph panels
- `GET /` - Health check

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
