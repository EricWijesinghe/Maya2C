# Deliberately the same output names as modules/node-pool, so the root module can
# fold both clouds into one map without special-casing either.

output "cluster_name" {
  description = "GKE cluster name for this region."
  value       = google_container_cluster.this.name
}

output "cluster_endpoint" {
  description = "Kubernetes API endpoint."
  value       = "https://${google_container_cluster.this.endpoint}"
}

output "network_id" {
  description = "VPC hosting this region's nodes."
  value       = google_compute_network.this.id
}

output "subnet_id" {
  description = "Subnet the node pool runs in."
  value       = google_compute_subnetwork.this.id
}

output "vpc_cidr" {
  description = "This region's primary CIDR, for peering routes and cross-region firewall rules."
  value       = google_compute_subnetwork.this.ip_cidr_range
}

output "node_service_account" {
  description = "Service account the workers run as."
  value       = google_service_account.node.email
}

output "location" {
  description = "Region, for `gcloud container clusters get-credentials`."
  value       = google_container_cluster.this.location
}
