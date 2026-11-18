// Typed errors and a Result type. No exception ever crosses the C boundary:
// every FFI failure arrives as a RustCallStatus and leaves as an Error value.
#pragma once

#include <cstdint>
#include <cstdlib>
#include <optional>
#include <string>
#include <utility>
#include <variant>

namespace maya2c {

enum class ErrorCode {
  // sdk-ffi `SdkError`, in its declaration order.
  WrongLength,
  Malformed,
  VerificationFailed,
  // The Rust side panicked, a moved-from handle was used, or an FFI reply
  // could not be decoded. Always a bug somewhere, never bad user input.
  Internal,
  // Amount text.
  AmountSyntax,
  AmountOverflow,
  AmountTooPrecise,
  // Address text. Names follow crates/address and the Python SDK's kinds.
  AddressLength,
  AddressMixedCase,
  AddressNoSeparator,
  AddressEmptyHrp,
  AddressInvalidCharacter,
  AddressBadChecksum,
  AddressVersion,
  AddressPayload,
  AddressWrongNetwork,
  AddressUnknownNetwork,
};

struct Error {
  ErrorCode code;
  std::string message;
  // Set only for ErrorCode::WrongLength.
  std::uint32_t expected = 0;
  std::uint32_t actual = 0;
};

// std::expected is C++23; this SDK targets C++20, so it carries its own.
// Calling value() on an error, or error() on a value, aborts: a programming
// error is not an input to recover from.
template <class T>
class [[nodiscard]] Result {
 public:
  Result(T value) : v_(std::in_place_index<0>, std::move(value)) {}         // NOLINT
  Result(Error error) : v_(std::in_place_index<1>, std::move(error)) {}     // NOLINT

  [[nodiscard]] bool has_value() const noexcept { return v_.index() == 0; }
  explicit operator bool() const noexcept { return has_value(); }

  T& value() & { require(true); return *std::get_if<0>(&v_); }
  const T& value() const& { require(true); return *std::get_if<0>(&v_); }
  T&& value() && { require(true); return std::move(*std::get_if<0>(&v_)); }
  const Error& error() const& { require(false); return *std::get_if<1>(&v_); }

  T* operator->() { return &value(); }
  const T* operator->() const { return &value(); }

 private:
  void require(bool want_value) const noexcept {
    if (has_value() != want_value) std::abort();
  }
  std::variant<T, Error> v_;
};

template <>
class [[nodiscard]] Result<void> {
 public:
  Result() = default;
  Result(Error error) : error_(std::move(error)) {}  // NOLINT

  [[nodiscard]] bool has_value() const noexcept { return !error_.has_value(); }
  explicit operator bool() const noexcept { return has_value(); }
  const Error& error() const& {
    if (!error_) std::abort();
    return *error_;
  }

 private:
  std::optional<Error> error_;
};

}  // namespace maya2c
