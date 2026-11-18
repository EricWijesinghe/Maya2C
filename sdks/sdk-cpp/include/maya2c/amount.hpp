// A 256-bit amount in base units, 24 decimals per coin (crates/amount,
// ADR-046 as amended by ADR-068). Integer arithmetic only: no float can
// represent 10^-24 exactly, and a wallet that rounds is a wallet that loses.
#pragma once

#include <array>
#include <compare>
#include <cstddef>
#include <cstdint>
#include <optional>
#include <span>
#include <string>
#include <string_view>

#include "maya2c/result.hpp"

namespace maya2c {

class Amount {
 public:
  static constexpr std::size_t kDecimals = 24;
  static constexpr std::size_t kBytes = 32;

  constexpr Amount() noexcept = default;

  static constexpr Amount from_u64(std::uint64_t value) noexcept {
    Amount a;
    a.limbs_[0] = static_cast<std::uint32_t>(value);
    a.limbs_[1] = static_cast<std::uint32_t>(value >> 32);
    return a;
  }

  static constexpr Amount max() noexcept {
    Amount a;
    a.limbs_.fill(0xFFFFFFFFu);
    return a;
  }

  // Little-endian, as crates/amount `to_le_bytes` / `from_le_bytes`.
  static Amount from_le_bytes(std::span<const std::uint8_t, kBytes> bytes) noexcept {
    Amount a;
    for (std::size_t i = 0; i < kBytes; ++i) {
      a.limbs_[i / 4] |= static_cast<std::uint32_t>(bytes[i]) << (8 * (i % 4));
    }
    return a;
  }

  [[nodiscard]] std::array<std::uint8_t, kBytes> to_le_bytes() const noexcept {
    std::array<std::uint8_t, kBytes> out{};
    for (std::size_t i = 0; i < kBytes; ++i) {
      out[i] = static_cast<std::uint8_t>(limbs_[i / 4] >> (8 * (i % 4)));
    }
    return out;
  }

  // An integer count of base units in decimal, as the node's RPC writes it.
  static Result<Amount> from_base_units(std::string_view text) {
    if (text.empty()) return syntax_error(text);
    Amount a;
    for (char c : text) {
      if (c < '0' || c > '9') return syntax_error(text);
      if (!a.mul_add_small(10, static_cast<std::uint32_t>(c - '0'))) {
        return Error{ErrorCode::AmountOverflow, "amount exceeds 2^256 - 1 base units"};
      }
    }
    return a;
  }

  // Whole coins with up to 24 decimals: "1", "0.5", "12.000000000000000000000001".
  // A 25th decimal is refused, never rounded.
  static Result<Amount> parse_coins(std::string_view text) {
    const auto dot = text.find('.');
    const auto whole = text.substr(0, dot);
    const auto frac = dot == std::string_view::npos ? std::string_view{} : text.substr(dot + 1);
    if (whole.empty() || (dot != std::string_view::npos && frac.empty())) {
      return syntax_error(text);
    }
    for (char c : frac) {
      if (c < '0' || c > '9') return syntax_error(text);
    }
    if (frac.size() > kDecimals) {
      return Error{ErrorCode::AmountTooPrecise,
                   "amount has " + std::to_string(frac.size()) + " decimals; the limit is 24"};
    }
    std::string digits(whole);
    digits.append(frac);
    digits.append(kDecimals - frac.size(), '0');
    auto parsed = from_base_units(digits);
    if (!parsed && parsed.error().code == ErrorCode::AmountSyntax) return syntax_error(text);
    return parsed;
  }

  [[nodiscard]] std::string to_base_units() const {
    if (is_zero()) return "0";
    Amount rest = *this;
    std::string out;
    while (!rest.is_zero()) out.insert(out.begin(), static_cast<char>('0' + rest.div_small(10)));
    return out;
  }

  // Always exactly 24 decimals, so the text is unambiguous and round-trips.
  [[nodiscard]] std::string to_coins() const {
    std::string base = to_base_units();
    if (base.size() <= kDecimals) base.insert(0, kDecimals + 1 - base.size(), '0');
    base.insert(base.size() - kDecimals, 1, '.');
    return base;
  }

  [[nodiscard]] constexpr bool is_zero() const noexcept {
    for (auto l : limbs_) {
      if (l != 0) return false;
    }
    return true;
  }

  [[nodiscard]] std::optional<Amount> checked_add(const Amount& other) const noexcept {
    Amount out;
    std::uint64_t carry = 0;
    for (std::size_t i = 0; i < kLimbs; ++i) {
      const std::uint64_t sum = std::uint64_t{limbs_[i]} + other.limbs_[i] + carry;
      out.limbs_[i] = static_cast<std::uint32_t>(sum);
      carry = sum >> 32;
    }
    if (carry != 0) return std::nullopt;
    return out;
  }

  [[nodiscard]] std::optional<Amount> checked_sub(const Amount& other) const noexcept {
    if (*this < other) return std::nullopt;
    Amount out;
    std::uint64_t borrow = 0;
    for (std::size_t i = 0; i < kLimbs; ++i) {
      const std::uint64_t sub = std::uint64_t{other.limbs_[i]} + borrow;
      borrow = limbs_[i] < sub ? 1 : 0;
      out.limbs_[i] = static_cast<std::uint32_t>((std::uint64_t{limbs_[i]} + (borrow << 32)) - sub);
    }
    return out;
  }

  friend constexpr bool operator==(const Amount&, const Amount&) noexcept = default;
  friend constexpr std::strong_ordering operator<=>(const Amount& a, const Amount& b) noexcept {
    for (std::size_t i = kLimbs; i-- > 0;) {
      if (a.limbs_[i] != b.limbs_[i]) return a.limbs_[i] <=> b.limbs_[i];
    }
    return std::strong_ordering::equal;
  }

 private:
  static constexpr std::size_t kLimbs = 8;

  static Error syntax_error(std::string_view text) {
    return Error{ErrorCode::AmountSyntax,
                 "\"" + std::string(text) + "\" is not a decimal amount (digits, at most one '.')"};
  }

  // this = this * mul + add; false if the result would not fit in 256 bits.
  bool mul_add_small(std::uint32_t mul, std::uint32_t add) noexcept {
    std::uint64_t carry = add;
    for (auto& limb : limbs_) {
      const std::uint64_t v = std::uint64_t{limb} * mul + carry;
      limb = static_cast<std::uint32_t>(v);
      carry = v >> 32;
    }
    return carry == 0;
  }

  // this = this / div; returns the remainder.
  std::uint32_t div_small(std::uint32_t div) noexcept {
    std::uint64_t rem = 0;
    for (std::size_t i = kLimbs; i-- > 0;) {
      const std::uint64_t cur = (rem << 32) | limbs_[i];
      limbs_[i] = static_cast<std::uint32_t>(cur / div);
      rem = cur % div;
    }
    return static_cast<std::uint32_t>(rem);
  }

  std::array<std::uint32_t, kLimbs> limbs_{};  // least significant first
};

}  // namespace maya2c
