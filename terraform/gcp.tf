# Maya2C on GCP: the same three regions, the same module-per-region shape as
# main.tf does on AWS.
#
# ## Why this is opt-in
#
# `enable_gcp` defaults to false. A fleet does not need both clouds to be
# multi-region, and turning GCP on doubles the standing cost. Making it a flag
# means an AWS-only operator never has to hold GCP credentials, and a plan run
# without them does not fail.
#
# ## CIDRs
#
# Disjoint from the AWS ranges (10.10/20/30) and from each other, because the
# whole point of running in both clouds is that the two can eventually route to
# one another. Ranges chosen now that overlap are ranges that cannot be peered
# later without a rebuild.
#
#   region  primary        pods            services       control plane
#   us      10.40.0.0/16   10.44.0.0/14    10.48.0.0/20   10.49.0.0/28
#   eu      10.50.0.0/16   10.54.0.0/14    10.58.0.0/20   10.59.0.0/28
#   asia    10.60.0.0/16   10.64.0.0/14    10.68.0.0/20   10.69.0.0/28

module "gcp_us" {
  source = "./modules/gke-node-pool"
  count  = var.enable_gcp ? 1 : 0

  providers = {
    google = google.us
  }

  region_key     = "gcp-us"
  chain_id       = var.chain_id
  region         = "us-central1"
  zones          = ["us-central1-a", "us-central1-b", "us-central1-c"]
  vpc_cidr       = "10.40.0.0/16"
  pods_cidr      = "10.44.0.0/14"
  services_cidr  = "10.48.0.0/20"
  master_cidr    = "10.49.0.0/28"
  nodes_per_zone = var.gcp_nodes_per_zone
  labels         = var.gcp_labels
}

module "gcp_eu" {
  source = "./modules/gke-node-pool"
  count  = var.enable_gcp ? 1 : 0

  providers = {
    google = google.eu
  }

  region_key     = "gcp-eu"
  chain_id       = var.chain_id
  region         = "europe-west1"
  zones          = ["europe-west1-b", "europe-west1-c", "europe-west1-d"]
  vpc_cidr       = "10.50.0.0/16"
  pods_cidr      = "10.54.0.0/14"
  services_cidr  = "10.58.0.0/20"
  master_cidr    = "10.59.0.0/28"
  nodes_per_zone = var.gcp_nodes_per_zone
  labels         = var.gcp_labels
}

module "gcp_asia" {
  source = "./modules/gke-node-pool"
  count  = var.enable_gcp ? 1 : 0

  providers = {
    google = google.asia
  }

  region_key     = "gcp-asia"
  chain_id       = var.chain_id
  region         = "asia-southeast1"
  zones          = ["asia-southeast1-a", "asia-southeast1-b", "asia-southeast1-c"]
  vpc_cidr       = "10.60.0.0/16"
  pods_cidr      = "10.64.0.0/14"
  services_cidr  = "10.68.0.0/20"
  master_cidr    = "10.69.0.0/28"
  nodes_per_zone = var.gcp_nodes_per_zone
  labels         = var.gcp_labels
}
