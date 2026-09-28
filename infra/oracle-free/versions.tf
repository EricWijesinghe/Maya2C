# A public testnet seed on Oracle Cloud's Always Free tier.
#
# Why this exists beside infra/terraform/: that fleet is twelve r6i.4xlarge
# across three AWS regions, about $9,000 a month. The first public testnet is
# one seed validator at $0 (ADR-032). This is the smallest thing that puts a
# real node on the internet, not a cut-down copy of the fleet.
#
# State is local on purpose. One VM, one operator: a remote backend would be a
# second account holding a credential, for nothing a laptop backup does not do.
# Keep terraform.tfstate out of git (the .gitignore here does) and back it up.
terraform {
  required_version = ">= 1.9"

  required_providers {
    oci = {
      source  = "oracle/oci"
      version = "~> 9.3"
    }
  }
}

# Credentials come from ~/.oci/config (`oci setup config`), never from a
# variable: a key in a .tfvars file is a key one `git add .` from public.
provider "oci" {
  config_file_profile = var.oci_profile
  region              = var.region
}
