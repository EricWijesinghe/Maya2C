/* Arm MPS2+ AN505 (Cortex-M33) as QEMU models it, secure aliases:
   4 MiB SSRAM1 for code, 4 MiB SSRAM3 for data. The stack starts at the top of
   RAM and grows down, leaving room for ML-DSA-65's stack-allocated state. */
MEMORY
{
  FLASH : ORIGIN = 0x10000000, LENGTH = 4M
  RAM   : ORIGIN = 0x38000000, LENGTH = 4M
}
