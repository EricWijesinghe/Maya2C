locals {
  # libp2p for validator 0 as infra/testnet-vm/install.sh lays it out
  # (3110i), and the only node port the internet reaches. RPC (3200i) binds
  # 127.0.0.1 and gets no rule: it is unauthenticated and serves
  # get_mining_candidate and submit_block.
  p2p_port = 31100
  # Public on the seed, matching install.sh's MAYA2C-IN chain: the chat relay
  # (a relay nobody can reach is useless), and the gateway -- behind Caddy on
  # 80/443 with a domain, plain HTTP on 8080 without one.
  public_ports = concat([4001], var.domain == "" ? [8080] : [80, 443])
  vcn_cidr     = "10.42.0.0/16"
}

data "oci_identity_availability_domains" "ads" {
  compartment_id = var.compartment_ocid
}

# Newest Canonical Ubuntu 24.04 image built for Ampere (aarch64).
data "oci_core_images" "ubuntu_arm" {
  compartment_id           = var.compartment_ocid
  operating_system         = "Canonical Ubuntu"
  operating_system_version = "24.04"
  shape                    = "VM.Standard.A1.Flex"
  sort_by                  = "TIMECREATED"
  sort_order               = "DESC"
}

resource "oci_core_vcn" "maya" {
  compartment_id = var.compartment_ocid
  cidr_blocks    = [local.vcn_cidr]
  display_name   = "maya2c-testnet"
  dns_label      = "maya"
}

resource "oci_core_internet_gateway" "maya" {
  compartment_id = var.compartment_ocid
  vcn_id         = oci_core_vcn.maya.id
  display_name   = "maya2c-igw"
}

resource "oci_core_route_table" "maya" {
  compartment_id = var.compartment_ocid
  vcn_id         = oci_core_vcn.maya.id
  display_name   = "maya2c-routes"

  route_rules {
    destination       = "0.0.0.0/0"
    network_entity_id = oci_core_internet_gateway.maya.id
  }
}

resource "oci_core_security_list" "maya" {
  compartment_id = var.compartment_ocid
  vcn_id         = oci_core_vcn.maya.id
  display_name   = "maya2c-seed"

  egress_security_rules {
    destination = "0.0.0.0/0"
    protocol    = "all"
  }

  # SSH from the operator only; variables.tf refuses 0.0.0.0/0.
  ingress_security_rules {
    source   = var.operator_cidr
    protocol = "6"
    tcp_options {
      min = 22
      max = 22
    }
  }

  ingress_security_rules {
    source   = "0.0.0.0/0"
    protocol = "6"
    tcp_options {
      min = local.p2p_port
      max = local.p2p_port
    }
  }

  dynamic "ingress_security_rules" {
    for_each = local.public_ports
    content {
      source   = "0.0.0.0/0"
      protocol = "6"
      tcp_options {
        min = ingress_security_rules.value
        max = ingress_security_rules.value
      }
    }
  }
}

resource "oci_core_subnet" "maya" {
  compartment_id    = var.compartment_ocid
  vcn_id            = oci_core_vcn.maya.id
  cidr_block        = cidrsubnet(local.vcn_cidr, 8, 1)
  display_name      = "maya2c-public"
  dns_label         = "seed"
  route_table_id    = oci_core_route_table.maya.id
  security_list_ids = [oci_core_security_list.maya.id]
}

resource "oci_core_instance" "seed" {
  compartment_id      = var.compartment_ocid
  availability_domain = data.oci_identity_availability_domains.ads.availability_domains[0].name
  display_name        = "maya2c-seed-1"
  shape               = "VM.Standard.A1.Flex"

  shape_config {
    ocpus         = var.ocpus
    memory_in_gbs = var.memory_gb
  }

  source_details {
    source_type             = "image"
    source_id               = data.oci_core_images.ubuntu_arm.images[0].id
    boot_volume_size_in_gbs = var.boot_volume_gb
  }

  create_vnic_details {
    subnet_id        = oci_core_subnet.maya.id
    assign_public_ip = true
  }

  metadata = {
    ssh_authorized_keys = var.ssh_public_key
    user_data = base64encode(templatefile("${path.module}/cloud-init.yaml", {
      chain_id   = var.chain_id
      git_commit = var.git_commit
      domain     = var.domain
      acme_email = var.acme_email
    }))
  }

  # A new image, commit or cloud-init must not silently replace a validator:
  # the validator key lives on this boot volume and would be destroyed with
  # it. Upgrades are a re-run of install.sh on the VM, not a new instance.
  lifecycle {
    ignore_changes = [source_details[0].source_id, metadata["user_data"]]
  }
}
