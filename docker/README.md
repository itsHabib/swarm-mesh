# Swarm Mesh Monitoring Setup

This directory contains Docker Compose configuration for monitoring your swarm-mesh nodes using Prometheus and Grafana.

## Services

- **Prometheus**: Metrics collection and storage (port 9090)
- **Grafana**: Metrics visualization and dashboards (port 3000)

## Quick Start

1. Make sure your swarm-mesh nodes are running and exposing metrics on their configured ports (8080, 8081, 8082, etc.)

2. Start the mesh-registry service for dynamic node discovery:
   ```bash
   cargo run --bin mesh-registry
   ```

3. Start the monitoring stack:
   ```bash
   cd docker
   docker-compose up -d
   ```

4. Access the services:
   - Grafana: http://localhost:3000 (admin/admin)
   - Prometheus: http://localhost:9090
   - Mesh Registry: http://localhost:5000

## Configuration

### Dynamic Node Discovery

The monitoring setup now uses **dynamic service discovery** via the mesh-registry service. Prometheus automatically discovers active nodes by querying the mesh-registry service at `http://host.docker.internal:5000/prometheus/targets`.

**No manual configuration required!** New nodes are automatically discovered when they:
1. Start up and register with the mesh-registry service
2. Send periodic heartbeats to maintain their registration

### Manual Node Configuration (Legacy)

If you prefer static configuration, you can edit `prometheus/prometheus.yml` and replace the `http_sd_configs` section with `static_configs`:

```yaml
- job_name: 'swarm-mesh-nodes'
  static_configs:
    - targets: 
        - 'host.docker.internal:8080'  # Node 1
        - 'host.docker.internal:8081'  # Node 2
        # Add more nodes as needed
```

### Grafana

- Default login: admin/admin
- **Prometheus datasource** is automatically configured for metrics
- **Mesh-Registry JSON datasource** is automatically configured for node graph visualization
- The node graph panel now uses live data from the mesh-registry service
- Create custom dashboards for your swarm-mesh metrics

## Stopping the Stack

```bash
docker-compose down
```

To remove all data:
```bash
docker-compose down -v
``` 