// The uniffi C ABI of sdk-ffi, and the plumbing that makes it safe to call.
//
// sdk-ffi is a uniffi crate: its C surface is the scaffolding uniffi
// generates, declared in sdks/sdk-ffi/bindings/swift/maya_sdk_ffiFFI.h. That
// header uses clang's _Nullable/_Nonnull, which GCC and MSVC reject, so the
// functions this SDK calls are re-declared here with the same signatures.
// The wire rules (what uniffi 0.32, contract 30, puts in a RustBuffer) are the
// ones the generated Python in bindings/python/maya_sdk_ffi.py implements:
//   - Vec<u8> argument/return: i32 big-endian length, then the bytes;
//   - String argument/return: the raw UTF-8, no length prefix;
//   - SdkError: i32 big-endian variant (1-based), then its fields in order,
//     u32 big-endian, a String field as i32 length + UTF-8;
//   - a method takes a *cloned* object handle, which Rust consumes.
//   - every RustBuffer argument is consumed by Rust; every returned one is
//     ours to free with ffi_maya_sdk_ffi_rustbuffer_free.
#pragma once

#include <climits>
#include <cstddef>
#include <cstdint>
#include <span>
#include <string>
#include <type_traits>
#include <utility>
#include <vector>

#include "maya2c/result.hpp"
#include "maya2c/secure_zero.hpp"

namespace maya2c::detail {

namespace ffi {
extern "C" {
struct RustBuffer {
  std::uint64_t capacity;
  std::uint64_t len;
  std::uint8_t* data;
};
struct ForeignBytes {
  std::int32_t len;
  const std::uint8_t* data;
};
struct RustCallStatus {
  std::int8_t code;
  RustBuffer errorBuf;
};

std::uint64_t uniffi_maya_sdk_ffi_fn_clone_signingkey(std::uint64_t handle, RustCallStatus* status);
void uniffi_maya_sdk_ffi_fn_free_signingkey(std::uint64_t handle, RustCallStatus* status);
std::uint64_t uniffi_maya_sdk_ffi_fn_constructor_signingkey_from_seed(RustBuffer seed,
                                                                       RustCallStatus* status);
std::uint64_t uniffi_maya_sdk_ffi_fn_constructor_signingkey_generate(RustCallStatus* status);
RustBuffer uniffi_maya_sdk_ffi_fn_method_signingkey_address(std::uint64_t ptr, RustCallStatus* status);
RustBuffer uniffi_maya_sdk_ffi_fn_method_signingkey_public_key(std::uint64_t ptr,
                                                              RustCallStatus* status);
RustBuffer uniffi_maya_sdk_ffi_fn_method_signingkey_sign(std::uint64_t ptr, RustBuffer message,
                                                        RustCallStatus* status);
RustBuffer uniffi_maya_sdk_ffi_fn_func_address_from_public_key(RustBuffer public_key,
                                                              RustCallStatus* status);
void uniffi_maya_sdk_ffi_fn_func_verify(RustBuffer public_key, RustBuffer message,
                                        RustBuffer signature, RustCallStatus* status);
RustBuffer ffi_maya_sdk_ffi_rustbuffer_from_bytes(ForeignBytes bytes, RustCallStatus* status);
void ffi_maya_sdk_ffi_rustbuffer_free(RustBuffer buf, RustCallStatus* status);
std::uint32_t ffi_maya_sdk_ffi_uniffi_contract_version(void);
}  // extern "C"
}  // namespace ffi

// The contract version the declarations above were written against
// (`bindings_contract_version` in the generated Python).
inline constexpr std::uint32_t kUniffiContractVersion = 30;

inline constexpr std::int8_t kCallSuccess = 0;
inline constexpr std::int8_t kCallError = 1;
inline constexpr std::int8_t kCallPanic = 2;

// A RustBuffer this side owns. Freed through the FFI, never with free().
class OwnedBuffer {
 public:
  OwnedBuffer() noexcept = default;
  // `secret`: the bytes are wiped before the buffer goes back to Rust's allocator.
  explicit OwnedBuffer(ffi::RustBuffer buf, bool secret = false) noexcept
      : buf_(buf), owned_(true), secret_(secret) {}
  ~OwnedBuffer() { reset(); }
  OwnedBuffer(const OwnedBuffer&) = delete;
  OwnedBuffer& operator=(const OwnedBuffer&) = delete;
  OwnedBuffer(OwnedBuffer&& other) noexcept
      : buf_(std::exchange(other.buf_, ffi::RustBuffer{})),
        owned_(std::exchange(other.owned_, false)),
        secret_(other.secret_) {}
  OwnedBuffer& operator=(OwnedBuffer&& other) noexcept {
    if (this != &other) {
      reset();
      buf_ = std::exchange(other.buf_, ffi::RustBuffer{});
      owned_ = std::exchange(other.owned_, false);
      secret_ = other.secret_;
    }
    return *this;
  }

  [[nodiscard]] std::span<const std::uint8_t> bytes() const noexcept {
    if (buf_.data == nullptr) return {};
    return {buf_.data, static_cast<std::size_t>(buf_.len)};
  }

  // Hands the buffer to a Rust call that consumes it.
  [[nodiscard]] ffi::RustBuffer release() noexcept {
    owned_ = false;
    return std::exchange(buf_, ffi::RustBuffer{});
  }

  void reset() noexcept {
    if (!owned_) return;
    if (secret_ && buf_.data != nullptr) secure_zero(buf_.data, static_cast<std::size_t>(buf_.len));
    ffi::RustCallStatus status{};
    ffi::ffi_maya_sdk_ffi_rustbuffer_free(buf_, &status);
    buf_ = ffi::RustBuffer{};
    owned_ = false;
  }

 private:
  ffi::RustBuffer buf_{};
  bool owned_ = false;
  bool secret_ = false;
};

inline Error internal(std::string message) { return Error{ErrorCode::Internal, std::move(message)}; }

class Reader {
 public:
  explicit Reader(std::span<const std::uint8_t> buf) noexcept : buf_(buf) {}
  bool u32(std::uint32_t& out) noexcept {
    if (buf_.size() - pos_ < 4) return false;
    out = (std::uint32_t{buf_[pos_]} << 24) | (std::uint32_t{buf_[pos_ + 1]} << 16) |
          (std::uint32_t{buf_[pos_ + 2]} << 8) | std::uint32_t{buf_[pos_ + 3]};
    pos_ += 4;
    return true;
  }
  bool bytes(std::size_t n, std::span<const std::uint8_t>& out) noexcept {
    if (buf_.size() - pos_ < n) return false;
    out = buf_.subspan(pos_, n);
    pos_ += n;
    return true;
  }
  [[nodiscard]] bool at_end() const noexcept { return pos_ == buf_.size(); }

 private:
  std::span<const std::uint8_t> buf_;
  std::size_t pos_ = 0;
};

// sdk-ffi `SdkError` from its serialised form. Anything unexpected is
// Internal: guessing a variant would hand the caller a reason that is false.
inline Error decode_sdk_error(std::span<const std::uint8_t> buf) {
  Reader r(buf);
  std::uint32_t variant = 0;
  if (!r.u32(variant)) return internal("sdk-ffi error buffer is truncated");
  switch (variant) {
    case 1: {
      std::uint32_t expected = 0;
      std::uint32_t actual = 0;
      if (!r.u32(expected) || !r.u32(actual)) return internal("sdk-ffi error buffer is truncated");
      return Error{ErrorCode::WrongLength,
                   "expected " + std::to_string(expected) + " bytes, got " + std::to_string(actual),
                   expected, actual};
    }
    case 2: {
      std::uint32_t len = 0;
      std::span<const std::uint8_t> what;
      if (!r.u32(len) || len > INT32_MAX || !r.bytes(len, what)) {
        return internal("sdk-ffi error buffer is truncated");
      }
      return Error{ErrorCode::Malformed, "malformed " + std::string(what.begin(), what.end())};
    }
    case 3:
      return Error{ErrorCode::VerificationFailed, "signature did not verify"};
    default:
      return internal("sdk-ffi returned unknown SdkError variant " + std::to_string(variant));
  }
}

// Turns a failed status into an Error, freeing its buffer.
inline Error status_error(ffi::RustCallStatus& status) {
  OwnedBuffer buf(status.errorBuf);
  status.errorBuf = ffi::RustBuffer{};
  if (status.code == kCallError) return decode_sdk_error(buf.bytes());
  if (status.code == kCallPanic && !buf.bytes().empty()) {
    const auto b = buf.bytes();
    return internal("sdk-ffi panicked: " + std::string(b.begin(), b.end()));
  }
  return internal("sdk-ffi failed with status " + std::to_string(status.code));
}

// Calls `fn(&status)` and returns its value or the mapped error.
template <class Fn>
auto rust_call(Fn&& fn) {
  using R = std::invoke_result_t<Fn&, ffi::RustCallStatus*>;
  ffi::RustCallStatus status{};
  if constexpr (std::is_void_v<R>) {
    fn(&status);
    if (status.code == kCallSuccess) return Result<void>{};
    return Result<void>{status_error(status)};
  } else {
    R out = fn(&status);
    if (status.code == kCallSuccess) return Result<R>{std::move(out)};
    return Result<R>{status_error(status)};
  }
}

inline Result<void> ensure_contract() {
  static const std::uint32_t found = ffi::ffi_maya_sdk_ffi_uniffi_contract_version();
  if (found == kUniffiContractVersion) return {};
  return internal("sdk-ffi uniffi contract version is " + std::to_string(found) +
                  "; this SDK was written for " + std::to_string(kUniffiContractVersion));
}

// A Vec<u8> argument. `secret` wipes this side's scratch copy afterwards.
inline Result<OwnedBuffer> lower_bytes(std::span<const std::uint8_t> bytes, bool secret = false) {
  if (bytes.size() > static_cast<std::size_t>(INT32_MAX) - 4) {
    return Error{ErrorCode::Malformed, "malformed input: larger than the FFI's 2 GiB limit"};
  }
  const auto n = static_cast<std::uint32_t>(bytes.size());
  std::vector<std::uint8_t> scratch;
  scratch.reserve(bytes.size() + 4);
  for (int shift = 24; shift >= 0; shift -= 8) scratch.push_back(static_cast<std::uint8_t>(n >> shift));
  scratch.insert(scratch.end(), bytes.begin(), bytes.end());
  auto lowered = rust_call([&](ffi::RustCallStatus* s) {
    return ffi::ffi_maya_sdk_ffi_rustbuffer_from_bytes(
        ffi::ForeignBytes{static_cast<std::int32_t>(scratch.size()), scratch.data()}, s);
  });
  if (secret) secure_zero(scratch.data(), scratch.size());
  if (!lowered) return lowered.error();
  return OwnedBuffer(lowered.value(), secret);
}

// A Vec<u8> return value.
inline Result<std::vector<std::uint8_t>> lift_bytes(const OwnedBuffer& buf) {
  Reader r(buf.bytes());
  std::uint32_t len = 0;
  std::span<const std::uint8_t> body;
  if (!r.u32(len) || len > INT32_MAX || !r.bytes(len, body) || !r.at_end()) {
    return internal("sdk-ffi returned a malformed byte buffer");
  }
  return std::vector<std::uint8_t>(body.begin(), body.end());
}

// A String return value.
inline std::string lift_string(const OwnedBuffer& buf) {
  const auto b = buf.bytes();
  return std::string(b.begin(), b.end());
}

}  // namespace maya2c::detail
