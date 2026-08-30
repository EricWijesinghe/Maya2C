# One GCP region's network and GKE cluster.
#
# The counterpart to modules/node-pool, which does the same job on AWS. The two
# are separate modules rather than one module with a `cloud` switch: the resource
# graphs share no types, and a module full of `count = var.cloud == "aws" ? 1 : 0`
# is harder to read than two modules that each do one thing.
#
# What they *do* share is the input surface and the guard rails — the same
# mainnet block, the same three-zone floor, the same private-workers topology —
# so that a reviewer can diff the variable files and see that the two clouds are
# configured alike.

locals {
  name = "maya-${var.region_key}"

  labels = merge(var.labels, {
    "maya-region" = var.region_key
    # Chain ids may contain characters GCP labels reject, so the raw value is not
    # safe to interpolate here. It is enforced by the variable validation and
    # recorded in the cluster description instead.
    "part-of" = "maya2c"
  })
}

# --- Network -----------------------------------------------------------------

resource "google_compute_network" "this" {
  name = "${local.name}-vpc"

  # Auto mode would create a subnet in every region with ranges this project
  # does not control, which is how overlapping CIDRs get in by accident.
  auto_create_subnetworks = false
}

resource "google_compute_subnetwork" "this" {
  name          = "${local.name}-subnet"
  region        = var.region
  network       = google_compute_network.this.id
  ip_cidr_range = var.vpc_cidr

  # VPC-native. Pod IPs come from a routable secondary range rather than an
  # overlay, which is what makes cross-region and cross-cloud reachability a
  # routing question rather than an encapsulation one.
  secondary_ip_range {
    range_name    = "pods"
    ip_cidr_range = var.pods_cidr
  }

  secondary_ip_range {
    range_name    = "services"
    ip_cidr_range = var.services_cidr
  }

  private_ip_google_access = true
}

# Workers have no external addresses, so egress — image pulls, and the arbitrary
# outbound dials peer discovery makes — goes through Cloud NAT.
resource "google_compute_router" "this" {
  name    = "${local.name}-router"
  region  = var.region
  network = google_compute_network.this.id
}

resource "google_compute_router_nat" "this" {
  name   = "${local.name}-nat"
  router = google_compute_router.this.name
  region = var.region

  nat_ip_allocate_option             = "AUTO_ONLY"
  source_subnetwork_ip_ranges_to_nat = "ALL_SUBNETWORKS_ALL_IP_RANGES"

  log_config {
    enable = true
    filter = "ERRORS_ONLY"
  }
}

# --- Firewall ----------------------------------------------------------------
#
# Mirrors the three security group rules in the AWS module. Sources are
# expressed the same way: p2p from anywhere, everything else confined to this
# region's own ranges.

# A chain whose peers cannot dial in has one participant.
resource "google_compute_firewall" "p2p" {
  name        = "${local.name}-p2p"
  network     = google_compute_network.this.name
  description = "libp2p gossip and Kademlia"
  direction   = "INGRESS"

  source_ranges = ["0.0.0.0/0"]
  target_tags   = ["maya-node"]

  allow {
    protocol = "tcp"
    ports    = ["30333"]
  }
}

# RPC arrives through the ingress load balancer, which is where the rate limits
# live. A direct path to the port would route around them.
resource "google_compute_firewall" "rpc" {
  name        = "${local.name}-rpc"
  network     = google_compute_network.this.name
  description = "JSON-RPC and the aggregator market endpoint, in-VPC only"
  direction   = "INGRESS"

  source_ranges = [var.vpc_cidr, var.pods_cidr]
  target_tags   = ["maya-node"]

  allow {
    protocol = "tcp"
    ports    = ["8545", "8546"]
  }
}

# The exporter publishes peer topology and mempool contents. There is
# deliberately no rule widening this beyond the cluster's own ranges.
resource "google_compute_firewall" "metrics" {
  name        = "${local.name}-metrics"
  network     = google_compute_network.this.name
  description = "Prometheus exporter, in-VPC only"
  direction   = "INGRESS"

  source_ranges = [var.vpc_cidr, var.pods_cidr]
  target_tags   = ["maya-node"]

  allow {
    protocol = "tcp"
    ports    = ["9600"]
  }
}

# GKE's control plane health checks and the managed load balancers reach pods
# from fixed Google-owned ranges. Without this, LoadBalancer Services come up
# with every backend marked unhealthy and no indication why.
resource "google_compute_firewall" "health_checks" {
  name        = "${local.name}-health-checks"
  network     = google_compute_network.this.name
  description = "Google load balancer health probes"
  direction   = "INGRESS"

  source_ranges = ["35.191.0.0/16", "130.211.0.0/22"]
  target_tags   = ["maya-node"]

  allow {
    protocol = "tcp"
    ports    = ["8545", "9600", "30333"]
  }
}

# --- Cluster -----------------------------------------------------------------

resource "google_container_cluster" "this" {
  name        = local.name
  location    = var.region
  description = "Maya2C ${var.region_key} nodes, chain ${var.chain_id}"

  node_locations = var.zones

  network    = google_compute_network.this.id
  subnetwork = google_compute_subnetwork.this.id

  # The default pool cannot be configured, only replaced. Creating and
  # immediately removing it is the documented way to get a cluster whose only
  # node pool is the one declared below.
  remove_default_node_pool = true
  initial_node_count       = 1

  min_master_version = var.kubernetes_version

  ip_allocation_policy {
    cluster_secondary_range_name  = google_compute_subnetwork.this.secondary_ip_range[0].range_name
    services_secondary_range_name = google_compute_subnetwork.this.secondary_ip_range[1].range_name
  }

  # Workers have no public addresses, matching the AWS module's private subnets.
  # The control plane endpoint stays public so that operators and CI can reach
  # the API without a bastion; lock this down per environment if that trade
  # does not suit.
  private_cluster_config {
    enable_private_nodes    = true
    enable_private_endpoint = false
    # A distinct /28 that must not overlap the subnet, its secondary ranges, or
    # any other region. Deriving it from vpc_cidr would guarantee an overlap
    # with the primary range, which the API rejects late and unhelpfully.
    master_ipv4_cidr_block = var.master_cidr
  }

  # Workload Identity is the GCP counterpart to the Pod Identity the AWS module
  # uses for the EBS CSI driver. The persistent disk CSI driver is built into
  # GKE, so nothing here needs a role binding — but leaving Workload Identity
  # off would make every later addition reach for node service account keys.
  workload_identity_config {
    workload_pool = "${data.google_project.this.project_id}.svc.id.goog"
  }

  addons_config {
    # Explicit rather than defaulted: this is what backs the `maya-retain`
    # StorageClass in k8s/storage/gcp, and a cluster without it leaves every
    # seed PVC pending.
    gce_persistent_disk_csi_driver_config {
      enabled = true
    }
  }

  # Deleting a cluster that holds chain state should take a deliberate act, not
  # a stray `terraform destroy` in the wrong directory.
  deletion_protection = true

  resource_labels = local.labels
}

data "google_project" "this" {}

resource "google_service_account" "node" {
  account_id   = "${local.name}-node"
  display_name = "Maya2C ${var.region_key} worker nodes"
}

# Least privilege: workers write logs and metrics and pull images. They do not
# need the default compute service account's project-wide editor role.
resource "google_project_iam_member" "node" {
  for_each = toset([
    "roles/logging.logWriter",
    "roles/monitoring.metricWriter",
    "roles/monitoring.viewer",
    "roles/artifactregistry.reader",
  ])

  project = data.google_project.this.project_id
  role    = each.value
  member  = "serviceAccount:${google_service_account.node.email}"
}

resource "google_container_node_pool" "this" {
  name     = "${local.name}-seeds"
  location = var.region
  cluster  = google_container_cluster.this.name

  # Per zone, not in total. Three zones at one each is the three-node floor the
  # PodDisruptionBudget of minAvailable 2 requires.
  node_count = var.nodes_per_zone

  node_config {
    machine_type    = var.machine_type
    disk_size_gb    = var.disk_size_gb
    disk_type       = "pd-balanced"
    service_account = google_service_account.node.email

    oauth_scopes = ["https://www.googleapis.com/auth/cloud-platform"]

    # Matches the target_tags on the firewall rules above.
    tags = ["maya-node"]

    labels = local.labels

    workload_metadata_config {
      mode = "GKE_METADATA"
    }

    shielded_instance_config {
      enable_secure_boot          = true
      enable_integrity_monitoring = true
    }
  }

  management {
    auto_repair = true
    # Off deliberately. An unattended upgrade drains nodes on Google's schedule,
    # and a chain node evicted mid-reorg resyncs rather than restarts. Upgrades
    # go through the same one-at-a-time window as the AWS node group.
    auto_upgrade = false
  }

  upgrade_settings {
    # One at a time, matching aws_eks_node_group.update_config.max_unavailable.
    # Each node holds a PVC and a peer identity; rolling two at once can drop the
    # seed set below the PodDisruptionBudget floor.
    max_surge       = 1
    max_unavailable = 0
  }
}
