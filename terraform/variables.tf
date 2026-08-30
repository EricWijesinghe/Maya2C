variable "chain_id" {
  description = "Genesis chain id these nodes serve."
  type        = string
  default     = "maya-testnet"
}

variable "instance_type" {
  description = "Worker instance type in every region."
  type        = string
  default     = "m6i.xlarge"
}

variable "nodes_per_region" {
  description = "Workers per region."
  type        = number
  default     = 3
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