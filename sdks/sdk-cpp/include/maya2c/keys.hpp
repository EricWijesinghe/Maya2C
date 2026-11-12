// Hybrid post-quantum signing keys over sdk-ffi.
//
// The rule sdk-ffi is built around holds here too: the secret key never
// crosses the boundary. SigningKey is an owning handle to a key that lives in
// Rust (zeroized there on drop); it signs and reports its public key, and has
// no way to export the secret. The one secret this side holds is a Seed the
// caller brought, and Seed wipes itself.
#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <span>
#include <string>
#include <utility>
#include <vector>

#include "maya2c/address.hpp"
#include "maya2c/detail/ffi.hpp"
#include "maya2c/result.hpp"
#include "maya2c/secure_zero.hpp"

namespace maya2c {

// ML-DSA-65 (3,309) + SLH-DSA-SHA2-128s (7,856): sdks/sdk-ffi/src/lib.rs.
inline constexpr std::size_t kSignatureBytes = 11165;
inline constexpr std::size_t kSeedBytes = 32;

inline std::uint32_t ffi_contract_version() {
  return detail::ffi::ffi_maya_sdk_ffi_uniffi_contract_version();
}

// A 32-byte seed. Not copyable; wiped on destruction and when moved from.
class Seed {
 public:
  static Result<Seed> from_bytes(std::span<const std::uint8_t> bytes) {
    if (bytes.size() != kSeedBytes) {
      return Error{ErrorCode::WrongLength,
                   "expected 32 bytes, got " + std::to_string(bytes.size()), 32,
                   static_cast<std::uint32_t>(bytes.size())};
    }
    Seed s;
    for (std::size_t i = 0; i < kSeedBytes; ++i) s.bytes_[i] = bytes[i];
    return Result<Seed>(std::move(s));
  }

  ~Seed() { secure_zero(bytes_.data(), bytes_.size()); }
  Seed(const Seed&) = delete;
  Seed& operator=(const Seed&) = delete;
  Seed(Seed&& other) noexcept : bytes_(other.bytes_) { other.wipe(); }
  Seed& operator=(Seed&& other) noexcept {
    if (this != &other) {
      bytes_ = other.bytes_;
      other.wipe();
    }
    return *this;
  }

  [[nodiscard]] std::span<const std::uint8_t, kSeedBytes> expose() const noexcept { return bytes_; }

 private:
  Seed() = default;
  void wipe() noexcept { secure_zero(bytes_.data(), bytes_.size()); }
  std::array<std::uint8_t, kSeedBytes> bytes_{};
};

namespace detail {

inline Result<AccountId> account_id_from_hex(const std::string& hex) {
  const auto nibble = [](char c) -> int {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    return -1;
  };
  AccountId id{};
  if (hex.size() != 2 * id.size()) return internal("sdk-ffi returned an address of the wrong length");
  for (std::size_t i = 0; i < id.size(); ++i) {
    const int hi = nibble(hex[2 * i]);
    const int lo = nibble(hex[2 * i + 1]);
    if (hi < 0 || lo < 0) return internal("sdk-ffi returned a non-hex address");
    id[i] = static_cast<std::uint8_t>((hi << 4) | lo);
  }
  return id;
}

}  // namespace detail

class SigningKey {
 public:
  // Derives the key from the caller's seed. Deterministic: same seed, same key.
  static Result<SigningKey> from_seed(const Seed& seed) { return from_seed(seed.expose()); }

  // As above from raw bytes; sdk-ffi refuses anything but 32 (WrongLength).
  static Result<SigningKey> from_seed(std::span<const std::uint8_t> seed) {
    if (auto ok = detail::ensure_contract(); !ok) return ok.error();
    auto lowered = detail::lower_bytes(seed, /*secret=*/true);
    if (!lowered) return lowered.error();
    auto handle = detail::rust_call([&](detail::ffi::RustCallStatus* s) {
      return detail::ffi::uniffi_maya_sdk_ffi_fn_constructor_signingkey_from_seed(
          lowered.value().release(), s);
    });
    if (!handle) return handle.error();
    return SigningKey(handle.value());
  }

  // A fresh key from the operating system CSPRNG (inside Rust).
  static Result<SigningKey> generate() {
    if (auto ok = detail::ensure_contract(); !ok) return ok.error();
    auto handle = detail::rust_call([](detail::ffi::RustCallStatus* s) {
      return detail::ffi::uniffi_maya_sdk_ffi_fn_constructor_signingkey_generate(s);
    });
    if (!handle) return handle.error();
    return SigningKey(handle.value());
  }

  ~SigningKey() { reset(); }
  SigningKey(const SigningKey&) = delete;
  SigningKey& operator=(const SigningKey&) = delete;
  SigningKey(SigningKey&& other) noexcept : handle_(std::exchange(other.handle_, 0)) {}
  SigningKey& operator=(SigningKey&& other) noexcept {
    if (this != &other) {
      reset();
      handle_ = std::exchange(other.handle_, 0);
    }
    return *this;
  }

  // False after the key was moved from.
  [[nodiscard]] bool valid() const noexcept { return handle_ != 0; }

  // The public key in the chain's own encoding.
  [[nodiscard]] Result<std::vector<std::uint8_t>> public_key() const {
    auto buf = call_buffer([](std::uint64_t h, detail::ffi::RustCallStatus* s) {
      return detail::ffi::uniffi_maya_sdk_ffi_fn_method_signingkey_public_key(h, s);
    });
    if (!buf) return buf.error();
    return detail::lift_bytes(buf.value());
  }

  // The 32-byte account id this key controls.
  [[nodiscard]] Result<AccountId> account_id() const {
    auto buf = call_buffer([](std::uint64_t h, detail::ffi::RustCallStatus* s) {
      return detail::ffi::uniffi_maya_sdk_ffi_fn_method_signingkey_address(h, s);
    });
    if (!buf) return buf.error();
    return detail::account_id_from_hex(detail::lift_string(buf.value()));
  }

  [[nodiscard]] Result<Address> address(Network network) const {
    auto id = account_id();
    if (!id) return id.error();
    return Address::from_account_id(network, id.value());
  }

  // The 11,165-byte hybrid signature over `message`. Slow (the SLH-DSA half
  // dominates); interactive callers should sign off the UI thread.
  [[nodiscard]] Result<std::vector<std::uint8_t>> sign(std::span<const std::uint8_t> message) const {
    auto lowered = detail::lower_bytes(message);
    if (!lowered) return lowered.error();
    auto buf = call_buffer([&](std::uint64_t h, detail::ffi::RustCallStatus* s) {
      return detail::ffi::uniffi_maya_sdk_ffi_fn_method_signingkey_sign(h, lowered.value().release(), s);
    });
    if (!buf) return buf.error();
    return detail::lift_bytes(buf.value());
  }

 private:
  explicit SigningKey(std::uint64_t handle) noexcept : handle_(handle) {}

  void reset() noexcept {
    if (handle_ == 0) return;
    detail::ffi::RustCallStatus status{};
    detail::ffi::uniffi_maya_sdk_ffi_fn_free_signingkey(std::exchange(handle_, 0), &status);
  }

  // A uniffi method consumes the handle it is given, so each call gets a clone.
  template <class Fn>
  Result<detail::OwnedBuffer> call_buffer(Fn&& fn) const {
    if (handle_ == 0) return detail::internal("use of a moved-from SigningKey");
    auto clone = detail::rust_call([&](detail::ffi::RustCallStatus* s) {
      return detail::ffi::uniffi_maya_sdk_ffi_fn_clone_signingkey(handle_, s);
    });
    if (!clone) return clone.error();
    auto out = detail::rust_call(
        [&](detail::ffi::RustCallStatus* s) { return fn(clone.value(), s); });
    if (!out) return out.error();
    return detail::OwnedBuffer(out.value());
  }

  std::uint64_t handle_ = 0;
};

// Checks a hybrid signature: both the ML-DSA and the SLH-DSA half must verify.
inline Result<void> verify(std::span<const std::uint8_t> public_key,
                           std::span<const std::uint8_t> message,
                           std::span<const std::uint8_t> signature) {
  if (auto ok = detail::ensure_contract(); !ok) return ok.error();
  auto pk = detail::lower_bytes(public_key);
  if (!pk) return pk.error();
  auto msg = detail::lower_bytes(message);
  if (!msg) return msg.error();
  auto sig = detail::lower_bytes(signature);
  if (!sig) return sig.error();
  return detail::rust_call([&](detail::ffi::RustCallStatus* s) {
    detail::ffi::uniffi_maya_sdk_ffi_fn_func_verify(pk.value().release(), msg.value().release(),
                                                    sig.value().release(), s);
  });
}

// The account id a public key controls.
inline Result<AccountId> account_id_from_public_key(std::span<const std::uint8_t> public_key) {
  if (auto ok = detail::ensure_contract(); !ok) return ok.error();
  auto pk = detail::lower_bytes(public_key);
  if (!pk) return pk.error();
  auto buf = detail::rust_call([&](detail::ffi::RustCallStatus* s) {
    return detail::ffi::uniffi_maya_sdk_ffi_fn_func_address_from_public_key(pk.value().release(), s);
  });
  if (!buf) return buf.error();
  return detail::account_id_from_hex(detail::lift_string(detail::OwnedBuffer(buf.value())));
}

}  // namespace maya2c
