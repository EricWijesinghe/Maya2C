terraform {
  required_version = ">= 1.9"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
    google = {
      source  = "hashicorp/google"
      version = "~> 6.0"
    }
  }

  # Remote state, not local.
  #
  # Three regions in one state file means every apply is a fleet-wide operation,
  # and a fleet-wide operation run from a laptop with the only copy of the state
  # is one lost disk away from a cluster nobody can modify or destroy. The lock
  # table matters just as much: two engineers applying at once against local
  # state produces two divergent views of what exists.
  #
  # Backend blocks cannot take variables, so the bucket is filled in per
  # deployment with `terraform init -backend-config=...`. See
  # docs/mainnet-readiness.md for the bootstrap.
  backend "s3" {
    key = "maya2c/fleet.tfstate"
    # Native S3 locking; no DynamoDB table required from Terraform 1.10 onward.
    use_lockfile = true
    encrypt      = true
  }
}

# One aliased provider per region. The regions are fixed here rather than taken
# as variables: the availability zones in main.tf are region-specific, so a
# configurable region would let the two disagree.
provider "aws" {
  alias  = "us"
  region = "us-east-1"
}

provider "aws" {
  alias  = "eu"
  region = "eu-central-1"
}

provider "aws" {
  alias  = "asia"
  region = "ap-southeast-1"
}

# The GCP side of the fleet, wired up in gcp.tf. These are configured even when
# `enable_gcp` is false: a provider block costs nothing until a resource
# references it, and Terraform cannot conditionally declare one.
provider "google" {
  alias   = "us"
  project = var.gcp_project
  region  = "us-central1"
}

provider "google" {
  alias   = "eu"
  project = var.gcp_project
  region  = "europe-west1"
}

provider "google" {
  alias   = "asia"
  project = var.gcp_project
  region  = "asia-southeast1"
}
