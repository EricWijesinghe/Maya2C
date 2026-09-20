# The module declares which providers it needs but never configures them.
# A module that configures its own provider cannot be instantiated twice with
# different regions, which is exactly what this module exists to do.
terraform {
  required_version = ">= 1.9"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
  }
}