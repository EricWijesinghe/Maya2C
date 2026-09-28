output "seed_public_ip" {
  description = "Point seed1.maya2c.dev at this (DNS only, not proxied)."
  value       = oci_core_instance.seed.public_ip
}

output "next_steps" {
  value = "ssh ubuntu@${oci_core_instance.seed.public_ip} 'sudo cat /etc/maya2c/validator.pub' -- then follow infra/oracle-free/README.md step 4"
}
