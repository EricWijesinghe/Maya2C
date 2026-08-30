output "cluster_name" {
  description = "EKS cluster name for this region."
  value       = aws_eks_cluster.this.name
}

output "cluster_endpoint" {
  description = "Kubernetes API endpoint."
  value       = aws_eks_cluster.this.endpoint
}

output "vpc_id" {
  description = "VPC hosting this region's nodes."
  value       = aws_vpc.this.id
}

output "security_group_id" {
  description = "Security group applied to chain nodes."
  value       = aws_security_group.node.id
}

output "private_subnet_ids" {
  description = "Subnets the node group runs in."
  value       = aws_subnet.private[*].id
}

output "public_subnet_ids" {
  description = "Subnets load balancers are placed in."
  value       = aws_subnet.public[*].id
}

output "vpc_cidr" {
  description = "This region's CIDR, for peering routes and cross-region security group rules."
  value       = aws_vpc.this.cidr_block
}

output "nat_public_ips" {
  description = "Egress addresses of this region's nodes. Peers see outbound dials from here."
  value       = aws_eip.nat[*].public_ip
}