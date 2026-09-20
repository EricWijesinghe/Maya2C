# Cluster names and endpoints, for `aws eks update-kubeconfig` and
# `gcloud container clusters get-credentials` per region.
#
# The GCP entries collapse to an empty map when `enable_gcp` is false, so an
# AWS-only deployment gets exactly the output it had before the GCP modules
# existed rather than three nulls.
output "clusters" {
  description = "Cluster name and API endpoint per region, across both clouds."
  value = merge(
    {
      us   = { name = module.us.cluster_name, endpoint = module.us.cluster_endpoint }
      eu   = { name = module.eu.cluster_name, endpoint = module.eu.cluster_endpoint }
      asia = { name = module.asia.cluster_name, endpoint = module.asia.cluster_endpoint }
    },
    var.enable_gcp ? {
      gcp-us = {
        name     = module.gcp_us[0].cluster_name
        endpoint = module.gcp_us[0].cluster_endpoint
      }
      gcp-eu = {
        name     = module.gcp_eu[0].cluster_name
        endpoint = module.gcp_eu[0].cluster_endpoint
      }
      gcp-asia = {
        name     = module.gcp_asia[0].cluster_name
        endpoint = module.gcp_asia[0].cluster_endpoint
      }
    } : {}
  )
}

output "vpc_ids" {
  description = "VPC per region, for peering."
  value = merge(
    {
      us   = module.us.vpc_id
      eu   = module.eu.vpc_id
      asia = module.asia.vpc_id
    },
    var.enable_gcp ? {
      gcp-us   = module.gcp_us[0].network_id
      gcp-eu   = module.gcp_eu[0].network_id
      gcp-asia = module.gcp_asia[0].network_id
    } : {}
  )
}

# Every range the fleet occupies, in one place. Peering routes and cross-region
# firewall rules are written against this, and overlaps are the failure it
# exists to make visible.
output "cidrs" {
  description = "Primary CIDR per region, across both clouds."
  value = merge(
    {
      us   = module.us.vpc_cidr
      eu   = module.eu.vpc_cidr
      asia = module.asia.vpc_cidr
    },
    var.enable_gcp ? {
      gcp-us   = module.gcp_us[0].vpc_cidr
      gcp-eu   = module.gcp_eu[0].vpc_cidr
      gcp-asia = module.gcp_asia[0].vpc_cidr
    } : {}
  )
}

output "aws_nat_public_ips" {
  description = "Egress addresses of the AWS regions. Peers see outbound dials from here."
  value = {
    us   = module.us.nat_public_ips
    eu   = module.eu.nat_public_ips
    asia = module.asia.nat_public_ips
  }
}
