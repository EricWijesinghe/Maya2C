variable "chain_id" {
  description = "Genesis chain id these nodes serve."
  type        = string
  default     = "maya-testnet"
}

variable "instance_type" {
  description = "Worker instance type in every region. Ignored when bare_metal is true."
  type        = string

  # r6i.4xlarge: 16 vCPU, 128 GiB. The floor is set by the proof-of-work
  # dataset, not by request volume — a mainnet DAG is 4 GiB resident, and it
  # sits alongside RocksDB's block cache and write buffers. The previous
  # m6i.xlarge (16 GiB) cannot hold a mainnet dataset and a warm cache at the
  # same time, which shows up as a node that falls behind during a reorg rather
  # than as an error.
  default = "r6i.4xlarge"
}

variable "bare_metal" {
  description = <<-EOT
    Provision single-tenant hardware (r6i.metal) instead of virtualised instances.

    COST: roughly $5-7 per hour per instance. At the default four nodes in each
    of three regions that is approximately $50,000 per month, against roughly
    $9,000 for r6i.4xlarge. Stated here so the number is read before the apply
    rather than on the first invoice.

    Buys single-tenancy and no noisy neighbours. It does not buy more memory:
    r6i.metal and r6i.4xlarge differ in isolation and core count, and the
    dataset fits in both.
  EOT
  type        = bool
  default     = false
}

variable "nodes_per_region" {
  description = "Workers per region. Three regions, so the fleet is three times this."
  type        = number

  # Four, for twelve across the fleet. Three regions at three was the minimum
  # the PodDisruptionBudget allows; four leaves one node per region that can be
  # drained for a kernel upgrade without touching the budget.
  default = 4

  validation {
    condition     = var.nodes_per_region >= 3
    error_message = "Fewer than three workers per region cannot satisfy the seed PodDisruptionBudget of minAvailable 2."
  }
}

variable "tags" {
  description = "Tags applied to every AWS resource in every region."
  type        = map(string)
  default = {
    "maya.managed-by" = "terraform"
  }
}

variable "single_nat_gateway" {
  description = "Route each AWS region's private subnets through one NAT gateway instead of one per zone."
  type        = bool
  default     = true
}

# --- GCP ---------------------------------------------------------------------

variable "enable_gcp" {
  description = "Also build the three GKE regions in gcp.tf."
  type        = bool

  # Off by default so an AWS-only operator never needs GCP credentials, and a
  # plan run without them does not fail.
  default = false

  # Cross-variable validation, available from Terraform 1.9. Catching the
  # missing project at plan time beats discovering it when the first GKE
  # resource is submitted.
  validation {
    condition     = !var.enable_gcp || var.gcp_project != null
    error_message = "gcp_project must be set when enable_gcp is true."
  }
}

variable "gcp_project" {
  description = "GCP project the GKE clusters live in. Required when enable_gcp is true."
  type        = string
  default     = null

  validation {
    # Checked here rather than left to the provider: an unset project surfaces
    # as a per-resource API error halfway through an apply, by which point the
    # networks already exist.
    condition     = var.gcp_project == null || can(regex("^[a-z][a-z0-9-]{4,28}[a-z0-9]$", var.gcp_project))
    error_message = "gcp_project must be a valid GCP project id."
  }
}

variable "gcp_nodes_per_zone" {
  description = "Workers per zone in each GKE region. Three zones at one each meets the seed floor."
  type        = number
  default     = 1
}

variable "gcp_labels" {
  description = "Labels applied to every GCP resource. GCP rejects the dots and slashes the AWS tag map uses, so this is a separate value rather than a reused one."
  type        = map(string)
  default = {
    "managed-by" = "terraform"
  }
}