# The variable surface deliberately mirrors modules/node-pool. Two modules that
# take the same inputs can be reasoned about together; two that drift require the
# reader to hold both in their head to answer "is EU configured like US".

variable "region_key" {
  description = "Short region identifier used in resource names, e.g. \"us\"."
  type        = string

  validation {
    # Stricter than the AWS module's: GCP resource names must start with a
    # letter, end alphanumeric, and never contain uppercase.
    condition     = can(regex("^[a-z][a-z0-9-]{0,14}[a-z0-9]$", var.region_key))
    error_message = "region_key must be lowercase alphanumeric with hyphens, 2-16 characters, not ending in a hyphen."
  }
}

variable "chain_id" {
  description = "Genesis chain id these nodes serve."
  type        = string

  # Identical to the AWS module's guard, and identical on purpose. A mainnet
  # blocked in one cloud and permitted in the other is not a blocked mainnet.
  validation {
    condition     = !contains(["mainnet", "maya-mainnet"], var.chain_id)
    error_message = "Mainnet is blocked while the shielded pool uses an untrusted Groth16 setup. Run a ceremony and set prove::SETUP_IS_TRUSTED before deploying a value-bearing chain."
  }
}

variable "region" {
  description = "GCP region, e.g. \"us-central1\"."
  type        = string
}

variable "zones" {
  description = "Zones the regional node pool places nodes in."
  type        = list(string)

  # Same reasoning as the AWS module: the seeds carry a required zone
  # anti-affinity rule, so a third replica has nowhere to schedule with fewer
  # than three zones.
  validation {
    condition     = length(var.zones) >= 3
    error_message = "At least three zones are required by the seed anti-affinity rule."
  }
}

variable "vpc_cidr" {
  description = "Primary CIDR for this region's subnet. Must not overlap any other region, in either cloud."
  type        = string
}

variable "pods_cidr" {
  description = "Secondary range for pod IPs. VPC-native clusters require one."
  type        = string
}

variable "services_cidr" {
  description = "Secondary range for ClusterIP services."
  type        = string
}

variable "master_cidr" {
  description = "The /28 the private control plane endpoint lives in. Must not overlap any subnet or secondary range."
  type        = string

  validation {
    condition     = can(cidrnetmask(var.master_cidr)) && split("/", var.master_cidr)[1] == "28"
    error_message = "master_cidr must be a /28."
  }
}

variable "kubernetes_version" {
  description = "GKE release channel version prefix, e.g. \"1.31.\". Null tracks the channel default."
  type        = string
  default     = null
}

variable "machine_type" {
  description = "Worker machine type."
  type        = string
  # Four vCPU and 16 GiB, matching the AWS m6i.xlarge the other module defaults
  # to. RocksDB plus Argon2id verification falls behind a reorg on anything
  # smaller, and an asymmetric fleet makes latency comparisons meaningless.
  default = "n2-standard-4"
}

variable "nodes_per_zone" {
  description = "Workers in each zone. Total nodes is this times the zone count."
  type        = number
  default     = 1

  validation {
    condition     = var.nodes_per_zone >= 1
    error_message = "At least one worker per zone is required."
  }
}

variable "disk_size_gb" {
  description = "Boot disk per worker. Chain data lives on PersistentVolumes, not here."
  type        = number
  default     = 100
}

variable "labels" {
  description = "Labels applied to every resource that accepts them."
  type        = map(string)
  default     = {}

  validation {
    # GCP labels reject dots, slashes, and uppercase — all of which the AWS
    # module's tag map uses freely. Catching it here beats a provider error
    # thrown from whichever resource happens to be created first.
    condition = alltrue([
      for key, value in var.labels :
      can(regex("^[a-z][a-z0-9_-]{0,62}$", key)) && can(regex("^[a-z0-9_-]{0,63}$", value))
    ])
    error_message = "GCP labels must be lowercase alphanumeric with underscores or hyphens; no dots, slashes, or uppercase."
  }
}
