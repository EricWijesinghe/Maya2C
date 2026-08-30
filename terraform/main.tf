# Maya2C multi-region deployment.
#
# Three regions, one module. Each gets its own provider alias so that a single
# `terraform apply` reaches all three — regions are not separate states, because
# separate states make it possible for one region to drift a release behind
# without anything reporting it.
#
# ## Non-overlapping CIDRs
#
# The three VPC ranges are disjoint on purpose. Nodes gossip across regions, so
# any future VPC peering or transit gateway needs unambiguous routing; picking
# 10.0.0.0/16 three times works right up until the day the regions must talk
# privately, and then it cannot be fixed without rebuilding.

module "us" {
  source = "./modules/node-pool"

  providers = {
    aws = aws.us
  }

  region_key         = "us"
  chain_id           = var.chain_id
  vpc_cidr           = "10.10.0.0/16"
  availability_zones = ["us-east-1a", "us-east-1b", "us-east-1c"]
  instance_type      = var.instance_type
  node_count         = var.nodes_per_region
  single_nat_gateway = var.single_nat_gateway
  tags               = var.tags
}

module "eu" {
  source = "./modules/node-pool"

  providers = {
    aws = aws.eu
  }

  region_key         = "eu"
  chain_id           = var.chain_id
  vpc_cidr           = "10.20.0.0/16"
  availability_zones = ["eu-central-1a", "eu-central-1b", "eu-central-1c"]
  instance_type      = var.instance_type
  node_count         = var.nodes_per_region
  single_nat_gateway = var.single_nat_gateway
  tags               = var.tags
}

module "asia" {
  source = "./modules/node-pool"

  providers = {
    aws = aws.asia
  }

  region_key         = "asia"
  chain_id           = var.chain_id
  vpc_cidr           = "10.30.0.0/16"
  availability_zones = ["ap-southeast-1a", "ap-southeast-1b", "ap-southeast-1c"]
  instance_type      = var.instance_type
  node_count         = var.nodes_per_region
  single_nat_gateway = var.single_nat_gateway
  tags               = var.tags
}
