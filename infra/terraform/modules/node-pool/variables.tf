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

  # The node itself refuses to start on a value-bearing chain whose shielded
  # pool can run on an unaudited circuit. Terraform cannot read the genesis,
  # so a mainnet deploy needs the operator to state the pool is off (ADR-037);
  # the node still checks it, and a wrong answer surfaces as a refused start.
  validation {
    condition     = !contains(["mainnet", "maya-mainnet"], var.chain_id) || var.mainnet_shielded_pool_off
    error_message = "A value-bearing chain needs a genesis with the shielded pool off (ADR-037); confirm it with mainnet_shielded_pool_off = true. The node refuses any other mainnet genesis while pool::CIRCUIT_IS_AUDITED is false."
  }
}

variable "mainnet_shielded_pool_off" {
  description = "Set true only to deploy a value-bearing chain whose genesis keeps the shielded pool off (shielded_activation_height = u64::MAX, ADR-037)."
  type        = bool
  default     = false
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
