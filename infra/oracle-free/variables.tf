variable "oci_profile" {
  description = "Profile in ~/.oci/config to authenticate with."
  type        = string
  default     = "DEFAULT"
}

variable "region" {
  description = "OCI home region. Always Free Ampere capacity exists only in the home region."
  type        = string
}

variable "compartment_ocid" {
  description = "Compartment to create everything in (the tenancy OCID is the root compartment)."
  type        = string
}

variable "ssh_public_key" {
  description = "OpenSSH public key for the `ubuntu` user. The public half only."
  type        = string
}

variable "operator_cidr" {
  description = "The one address range allowed to reach SSH, e.g. your home IP as 203.0.113.7/32."
  type        = string

  # SSH open to the world on a validator is how a testnet seed becomes
  # someone else's. Refused here, before any provider call.
  validation {
    condition     = can(cidrhost(var.operator_cidr, 0)) && !contains(["0.0.0.0/0", "::/0"], var.operator_cidr)
    error_message = "operator_cidr must be a real range and not 0.0.0.0/0; SSH is not opened to the internet."
  }
}

variable "chain_id" {
  description = "Genesis chain id this seed serves."
  type        = string
  default     = "maya-testnet-1"

  # Same rule as infra/terraform/modules/node-pool: a value-bearing chain only
  # with a genesis that keeps the shielded pool off (ADR-037).
  validation {
    condition     = !contains(["mainnet", "maya-mainnet"], var.chain_id) || var.mainnet_shielded_pool_off
    error_message = "A value-bearing chain needs a genesis with the shielded pool off (ADR-037); confirm it with mainnet_shielded_pool_off = true."
  }

  # chain_id, domain and acme_email are interpolated into a shell line that
  # cloud-init runs as root. A quote in any of them would end the string and
  # run whatever followed, so each is limited to characters that cannot.
  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9_-]{0,31}$", var.chain_id))
    error_message = "chain_id: lowercase letters, digits, '-' and '_', at most 32 characters."
  }
}

variable "git_commit" {
  description = "Full commit SHA of EricWijesinghe/Maya2C to build on the VM. Pinned, never a branch."
  type        = string

  # A branch name would build whatever landed a minute before boot. A full
  # SHA is the only ref that names exactly one tree.
  validation {
    condition     = can(regex("^[0-9a-f]{40}$", var.git_commit))
    error_message = "git_commit must be a full 40-character commit SHA, not a branch or tag."
  }
}

variable "domain" {
  description = "Optional hostname for the API gateway's TLS (e.g. rpc.maya2c.dev). Empty: no gateway, 80/443 stay closed."
  type        = string
  default     = ""

  validation {
    condition     = var.domain == "" || can(regex("^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\\.)+[a-z]{2,63}$", var.domain))
    error_message = "domain must be a lowercase hostname such as rpc.maya2c.dev, or empty."
  }
}

variable "acme_email" {
  description = "Contact email for the gateway's Let's Encrypt certificate. Required when domain is set."
  type        = string
  default     = ""

  validation {
    condition     = var.acme_email == "" || can(regex("^[A-Za-z0-9._%+-]{1,64}@([A-Za-z0-9-]{1,63}\\.)+[A-Za-z]{2,63}$", var.acme_email))
    error_message = "acme_email must be a plain address (letters, digits, . _ % + - before the @), or empty."
  }
}

# Always Free allows 4 OCPU and 24 GB of Ampere A1 per tenancy in total, and
# 200 GB of block storage. These defaults use all of the compute and half the
# storage; going over any of them starts billing.
variable "ocpus" {
  type    = number
  default = 4

  validation {
    condition     = var.ocpus >= 1 && var.ocpus <= 4
    error_message = "Always Free covers at most 4 Ampere OCPUs."
  }
}

variable "memory_gb" {
  type    = number
  default = 24

  validation {
    condition     = var.memory_gb >= 6 && var.memory_gb <= 24
    error_message = "Always Free covers at most 24 GB of Ampere memory; the node wants at least 6."
  }
}

variable "boot_volume_gb" {
  type    = number
  default = 100

  validation {
    condition     = var.boot_volume_gb >= 50 && var.boot_volume_gb <= 200
    error_message = "Always Free covers 200 GB of block storage in total."
  }
}

check "gateway_needs_email" {
  assert {
    condition     = var.domain == "" || var.acme_email != ""
    error_message = "acme_email is required when domain is set."
  }
}

variable "mainnet_shielded_pool_off" {
  description = "Set true only to deploy a value-bearing chain whose genesis keeps the shielded pool off (shielded_activation_height = u64::MAX, ADR-037)."
  type        = bool
  default     = false
}
