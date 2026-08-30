# Declares its providers, configures none. A module that configured its own
# provider could not be instantiated three times with three regions, which is
# the entire reason this module exists.
terraform {
  required_version = ">= 1.9"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.0"
    }
  }
}
