# Swarm Mesh Monitoring Setup

This directory contains Docker Compose configuration for monitoring your swarm-mesh nodes using Prometheus and Grafana.

## Services

- **Prometheus**: Metrics collection and storage (port 9090)
- **Grafana**: Metrics visualization and dashboards (port 3000)

## Quick Start

1. Make sure your swarm-mesh nodes are running and exposing metrics on their configured ports (8080, 8081, 8082, etc.)

2. Start the monitoring stack:
   ```bash
   cd docker
   docker-compose up -d
   ```

3. Access the services:
   - Grafana: http://localhost:3000 (admin/admin)
   - Prometheus: http://localhost:9090

## Configuration

### Adding More Nodes

To monitor additional nodes, edit `prometheus/prometheus.yml` and add more targets to the `swarm-mesh-nodes` job:

```yaml
- job_name: 'swarm-mesh-nodes'
  static_configs:
    - targets: 
        - 'host.docker.internal:8080'  # Node 1
        - 'host.docker.internal:8081'  # Node 2
        - 'host.docker.internal:8082'  # Node 3
        - 'host.docker.internal:8083'  # Node 4 (new)
```

Then reload Prometheus configuration:
```bash
docker-compose restart prometheus
```

### Grafana

- Default login: admin/admin
- Prometheus datasource is automatically configured
- Create custom dashboards for your swarm-mesh metrics

## Stopping the Stack

```bash
docker-compose down
```

To remove all data:
```bash
docker-compose down -v
``` 