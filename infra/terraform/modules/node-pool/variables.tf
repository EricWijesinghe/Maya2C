variable "region_key" {
  description = "Short region identifier used in resource names, e.g. \"us\"."
  type        = string

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{0,15}$", var.region_key))
    error_message = "region_key must be lowercase alphanumeric with hyphens, at most 16 characters."
  }
}

variable "chain_id" {
  description = "Genesis chain id these nodes serve."
  type        = string

  # The node itself refuses to start on a value-bearing chain while the shielded
  # pool's circuit is unaudited. Catching it here
  # means the mistake surfaces at plan time rather than as a CrashLoopBackOff.
  validation {
    condition     = !contains(["mainnet", "maya-mainnet"], var.chain_id)
    error_message = "Mainnet is blocked while the shielded pool's circuit is unaudited. Have the joinsplit AIR independently audited and set pool::CIRCUIT_IS_AUDITED before deploying a value-bearing chain."
  }
}

variable "vpc_cidr" {
  description = "CIDR block for this region's VPC. Must not overlap other regions."
  type        = string
}

variable "availability_zones" {
  description = "AZs to spread nodes across."
  type        = list(string)

  # Seeds carry a required zone anti-affinity rule, so a third replica has
  # nowhere to schedule with fewer than three zones.
  validation {
    condition     = length(var.availability_zones) >= 3
    error_message = "At least three availability zones are required by the seed anti-affinity rule."
  }
}

variable "kubernetes_version" {
  description = "EKS control plane version."
  type        = string
  default     = "1.31"
}

variable "instance_type" {
  description = "Worker instance type."
  type        = string
  # RocksDB plus Argon2id proof-of-work verification is memory- and
  # CPU-hungry; smaller instances fall behind during a reorg.
  default = "m6i.xlarge"
}

variable "node_count" {
  description = "Workers per region."
  type        = number
  default     = 3

  validation {
    condition     = var.node_count >= 3
    error_message = "Fewer than three workers cannot satisfy the seed PodDisruptionBudget of minAvailable 2."
  }
}

variable "single_nat_gateway" {
  description = "Route every private subnet through one NAT gateway instead of one per zone."
  type        = bool

  # Nine NAT gateways across three regions is a standing charge measured in
  # hundreds of dollars a month, bought against the case where one zone's NAT
  # fails while its nodes keep running. Default to the cheap side and let a
  # value-bearing deployment opt into the expensive one.
  default = true
}

variable "pod_identity_agent_version" {
  description = "eks-pod-identity-agent addon version. Null selects the cluster default."
  type        = string
  default     = null
}

variable "ebs_csi_version" {
  description = "aws-ebs-csi-driver addon version. Null selects the cluster default."
  type        = string
  default     = null
}

variable "tags" {
  description = "Extra tags applied to every resource."
  type        = map(string)
  default     = {}
}
