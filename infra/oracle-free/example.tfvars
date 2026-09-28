# Copy to terraform.tfvars (gitignored) and fill in. No secret goes here:
# OCI credentials stay in ~/.oci/config, and the validator key is generated
# on the VM and never leaves it.
region           = "eu-frankfurt-1"
compartment_ocid = "ocid1.tenancy.oc1..replace-me"
ssh_public_key   = "ssh-ed25519 AAAA... you@laptop"
operator_cidr    = "203.0.113.7/32"
chain_id         = "maya-testnet-1"
git_commit       = "0000000000000000000000000000000000000000"
domain           = "" # e.g. "rpc.maya2c.dev" to run the TLS gateway
acme_email       = ""
