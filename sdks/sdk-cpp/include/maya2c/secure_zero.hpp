// Wiping secret bytes in a way the optimiser may not remove.
//
// sdk-ffi exports no zeroize function — by design no secret ever crosses
// it (see sdks/sdk-ffi/src/lib.rs) — so the bytes the caller holds on this
// side, a seed, are wiped here.
#pragma once

#include <atomic>
#include <cstddef>
#include <cstdint>

#if defined(__GLIBC__)
#include <string.h>  // explicit_bzero, glibc >= 2.25
#endif

namespace maya2c {

inline void secure_zero(void* data, std::size_t size) noexcept {
  if (data == nullptr || size == 0) return;
#if defined(__GLIBC__) && ((__GLIBC__ > 2) || (__GLIBC__ == 2 && __GLIBC_MINOR__ >= 25))
  ::explicit_bzero(data, size);
#else
  // SecureZeroMemory on Windows is this same volatile loop (RtlSecureZeroMemory);
  // writing it here keeps <windows.h> out of a public header.
  volatile auto* p = static_cast<volatile std::uint8_t*>(data);
  for (std::size_t i = 0; i < size; ++i) p[i] = 0;
#endif
  std::atomic_signal_fence(std::memory_order_seq_cst);
}

}  // namespace maya2c
