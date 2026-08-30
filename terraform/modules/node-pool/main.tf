# One region's network and Kubernetes cluster.
#
# The module is deliberately region-agnostic: US, EU, and Asia are three
# instantiations of this file with different variables, not three copies. Copies
# drift, and a security group rule that exists in two regions out of three is
# the kind of drift nobody notices until an incident.

locals {
  name = "maya-${var.region_key}"

  tags = merge(var.tags, {
    "maya.region"               = var.region_key
    "maya.chain"                = var.chain_id
    "app.kubernetes.io/part-of" = "maya2c"
  })
}

# --- Network -----------------------------------------------------------------

resource "aws_vpc" "this" {
  cidr_block           = var.vpc_cidr
  enable_dns_support   = true
  enable_dns_hostnames = true

  tags = merge(local.tags, { Name = "${local.name}-vpc" })
}

resource "aws_internet_gateway" "this" {
  vpc_id = aws_vpc.this.id
  tags   = merge(local.tags, { Name = "${local.name}-igw" })
}

# One subnet per availability zone. Seeds carry a required anti-affinity rule on
# the zone topology key, so fewer than three zones leaves a replica unschedulable
# rather than silently colocated.
resource "aws_subnet" "public" {
  count = length(var.availability_zones)

  vpc_id                  = aws_vpc.this.id
  availability_zone       = var.availability_zones[count.index]
  cidr_block              = cidrsubnet(var.vpc_cidr, 4, count.index)
  map_public_ip_on_launch = true

  tags = merge(local.tags, {
    Name                     = "${local.name}-public-${count.index}"
    "kubernetes.io/role/elb" = "1"
  })
}

resource "aws_route_table" "public" {
  vpc_id = aws_vpc.this.id

  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.this.id
  }

  tags = merge(local.tags, { Name = "${local.name}-public" })
}

resource "aws_route_table_association" "public" {
  count = length(aws_subnet.public)

  subnet_id      = aws_subnet.public[count.index].id
  route_table_id = aws_route_table.public.id
}

# Workers live here, not in the public subnets. A chain node with a public IP is
# directly reachable on every port the host has open, and the only port that
# should be world-reachable is 30333 — which arrives through a load balancer in
# the public subnets instead, where the target group is the only path in.
#
# Offset by 8 so the private /20s cannot collide with the public ones as the AZ
# count grows.
resource "aws_subnet" "private" {
  count = length(var.availability_zones)

  vpc_id            = aws_vpc.this.id
  availability_zone = var.availability_zones[count.index]
  cidr_block        = cidrsubnet(var.vpc_cidr, 4, count.index + 8)

  tags = merge(local.tags, {
    Name                              = "${local.name}-private-${count.index}"
    "kubernetes.io/role/internal-elb" = "1"
  })
}

# Egress for the private subnets. Nodes need it: peer discovery dials arbitrary
# addresses, and image pulls reach a registry.
#
# One NAT gateway by default rather than one per zone. Three per region across
# three regions is nine NAT gateways of standing charge, which is real money for
# an availability gain that only matters if an entire AZ's NAT fails while its
# nodes stay up. Set `single_nat_gateway = false` for a production mainnet where
# that trade flips.
resource "aws_eip" "nat" {
  count = var.single_nat_gateway ? 1 : length(var.availability_zones)

  domain = "vpc"
  tags   = merge(local.tags, { Name = "${local.name}-nat-${count.index}" })
}

resource "aws_nat_gateway" "this" {
  count = var.single_nat_gateway ? 1 : length(var.availability_zones)

  allocation_id = aws_eip.nat[count.index].id
  subnet_id     = aws_subnet.public[count.index].id

  tags = merge(local.tags, { Name = "${local.name}-nat-${count.index}" })

  depends_on = [aws_internet_gateway.this]
}

resource "aws_route_table" "private" {
  count = length(var.availability_zones)

  vpc_id = aws_vpc.this.id

  route {
    cidr_block = "0.0.0.0/0"
    # Collapses to the single gateway when `single_nat_gateway` is set, and
    # follows the zone otherwise.
    nat_gateway_id = aws_nat_gateway.this[var.single_nat_gateway ? 0 : count.index].id
  }

  tags = merge(local.tags, { Name = "${local.name}-private-${count.index}" })
}

resource "aws_route_table_association" "private" {
  count = length(aws_subnet.private)

  subnet_id      = aws_subnet.private[count.index].id
  route_table_id = aws_route_table.private[count.index].id
}

# --- Firewall ----------------------------------------------------------------

resource "aws_security_group" "node" {
  name        = "${local.name}-node"
  description = "Maya2C chain node"
  vpc_id      = aws_vpc.this.id

  tags = merge(local.tags, { Name = "${local.name}-node" })
}

# P2P is the one port that must be open to the world. A chain whose peers
# cannot dial in has one participant.
resource "aws_vpc_security_group_ingress_rule" "p2p" {
  security_group_id = aws_security_group.node.id
  description       = "libp2p gossip and Kademlia"
  cidr_ipv4         = "0.0.0.0/0"
  from_port         = 30333
  to_port           = 30333
  ip_protocol       = "tcp"
}

# RPC is reachable only from inside the VPC. Public access arrives through the
# ingress load balancer, which is where the rate limits live; exposing the port
# directly would route around them.
resource "aws_vpc_security_group_ingress_rule" "rpc" {
  security_group_id = aws_security_group.node.id
  description       = "JSON-RPC, load balancer only"
  cidr_ipv4         = var.vpc_cidr
  from_port         = 8545
  to_port           = 8545
  ip_protocol       = "tcp"
}

# The exporter publishes peer topology and mempool contents. It is never
# reachable from outside the VPC, and there is deliberately no rule widening it.
resource "aws_vpc_security_group_ingress_rule" "metrics" {
  security_group_id = aws_security_group.node.id
  description       = "Prometheus exporter, in-VPC only"
  cidr_ipv4         = var.vpc_cidr
  from_port         = 9600
  to_port           = 9600
  ip_protocol       = "tcp"
}

resource "aws_vpc_security_group_egress_rule" "all" {
  security_group_id = aws_security_group.node.id
  description       = "Peer discovery reaches arbitrary addresses"
  cidr_ipv4         = "0.0.0.0/0"
  ip_protocol       = "-1"
}

# --- Cluster -----------------------------------------------------------------

resource "aws_iam_role" "cluster" {
  name = "${local.name}-cluster"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "eks.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })

  tags = local.tags
}

resource "aws_iam_role_policy_attachment" "cluster" {
  role       = aws_iam_role.cluster.name
  policy_arn = "arn:aws:iam::aws:policy/AmazonEKSClusterPolicy"
}

resource "aws_eks_cluster" "this" {
  name     = local.name
  role_arn = aws_iam_role.cluster.arn
  version  = var.kubernetes_version

  # Both tiers: the control plane places its cross-account ENIs in the private
  # subnets, and load balancers need the public ones tagged and reachable.
  vpc_config {
    subnet_ids         = concat(aws_subnet.public[*].id, aws_subnet.private[*].id)
    security_group_ids = [aws_security_group.node.id]
  }

  # Without this the control plane logs nothing, and a failed rollout has no
  # record to read afterwards.
  enabled_cluster_log_types = ["api", "audit", "authenticator"]

  tags = local.tags

  depends_on = [aws_iam_role_policy_attachment.cluster]
}

resource "aws_iam_role" "node" {
  name = "${local.name}-node"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "ec2.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })

  tags = local.tags
}

resource "aws_iam_role_policy_attachment" "node" {
  for_each = toset([
    "arn:aws:iam::aws:policy/AmazonEKSWorkerNodePolicy",
    "arn:aws:iam::aws:policy/AmazonEKS_CNI_Policy",
    "arn:aws:iam::aws:policy/AmazonEC2ContainerRegistryReadOnly",
  ])

  role       = aws_iam_role.node.name
  policy_arn = each.value
}

resource "aws_eks_node_group" "this" {
  cluster_name    = aws_eks_cluster.this.name
  node_group_name = "${local.name}-seeds"
  node_role_arn   = aws_iam_role.node.arn
  subnet_ids      = aws_subnet.private[*].id
  instance_types  = [var.instance_type]

  scaling_config {
    desired_size = var.node_count
    min_size     = var.node_count
    max_size     = var.node_count
  }

  # One node at a time. Chain nodes hold a PVC and a peer identity; rolling two
  # at once can drop the seed set below the PodDisruptionBudget's floor.
  update_config {
    max_unavailable = 1
  }

  tags = local.tags

  depends_on = [aws_iam_role_policy_attachment.node]
}

# --- Storage -----------------------------------------------------------------
#
# The seeds' volumeClaimTemplate asks for the `maya-retain` StorageClass, which
# is backed by `ebs.csi.aws.com`. Nothing provides that driver by default: EKS
# ships CoreDNS, kube-proxy and the VPC CNI, and stops there. Without the addon
# below every seed PVC sits in `Pending` indefinitely and the StatefulSet never
# produces a ready pod — a failure whose only symptom is silence.

# Pod Identity rather than IRSA. IRSA needs an OIDC provider, a TLS thumbprint
# data source, and a trust policy keyed on a string-interpolated issuer URL;
# Pod Identity replaces all of it with a role the addon references directly.
resource "aws_eks_addon" "pod_identity" {
  cluster_name  = aws_eks_cluster.this.name
  addon_name    = "eks-pod-identity-agent"
  addon_version = var.pod_identity_agent_version

  # The agent is a DaemonSet, so it needs somewhere to run.
  depends_on = [aws_eks_node_group.this]

  tags = local.tags
}

resource "aws_iam_role" "ebs_csi" {
  name = "${local.name}-ebs-csi"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "pods.eks.amazonaws.com" }
      # TagSession as well as AssumeRole: Pod Identity attaches the pod's
      # identity as session tags, and the trust policy has to permit it or the
      # assume call fails with an unhelpful AccessDenied.
      Action = ["sts:AssumeRole", "sts:TagSession"]
    }]
  })

  tags = local.tags
}

resource "aws_iam_role_policy_attachment" "ebs_csi" {
  role       = aws_iam_role.ebs_csi.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonEBSCSIDriverPolicy"
}

resource "aws_eks_addon" "ebs_csi" {
  cluster_name  = aws_eks_cluster.this.name
  addon_name    = "aws-ebs-csi-driver"
  addon_version = var.ebs_csi_version

  pod_identity_association {
    role_arn        = aws_iam_role.ebs_csi.arn
    service_account = "ebs-csi-controller-sa"
  }

  # Volumes hold the chain and the node identity. A driver upgrade that wants to
  # replace a live PV should fail loudly rather than take the operator's silence
  # as consent.
  resolve_conflicts_on_update = "NONE"

  tags = local.tags

  depends_on = [
    aws_eks_addon.pod_identity,
    aws_iam_role_policy_attachment.ebs_csi,
  ]
}
