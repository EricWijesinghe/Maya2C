// Bech32m text form of an account address (BIP-350, ADR-081).
//
// Same layout as crates/address and sdks/python/maya2c_sdk/address.py:
// <hrp> "1" version-symbol 52-payload-symbols 6-checksum-symbols. The shared
// vectors are spec/address-vectors.json.
#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <span>
#include <string>
#include <string_view>
#include <vector>

#include "maya2c/result.hpp"

namespace maya2c {

enum class Network { Mainnet, Testnet, Devnet };

inline constexpr std::size_t kAccountIdBytes = 32;
using AccountId = std::array<std::uint8_t, kAccountIdBytes>;

namespace detail::bech32 {

inline constexpr std::string_view kCharset = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";
inline constexpr std::uint32_t kBech32mConst = 0x2BC830A3;
inline constexpr std::size_t kChecksumLen = 6;
inline constexpr std::size_t kMaxLen = 90;
inline constexpr std::uint8_t kVersion = 0;
inline constexpr std::size_t kPayloadSymbols = 52;
inline constexpr std::size_t kDataSymbols = 1 + kPayloadSymbols;

inline constexpr std::string_view hrp(Network n) noexcept {
  switch (n) {
    case Network::Mainnet: return "maya";
    case Network::Testnet: return "tmaya";
    case Network::Devnet: return "dmaya";
  }
  return "";
}

inline std::uint32_t polymod(std::string_view hrp_text, std::span<const std::uint8_t> data) {
  constexpr std::array<std::uint32_t, 5> kGen{0x3B6A57B2, 0x26508E6D, 0x1EA119FA, 0x3D4233DD,
                                              0x2A1462B3};
  std::uint32_t chk = 1;
  const auto step = [&](std::uint32_t v) {
    const std::uint32_t top = chk >> 25;
    chk = ((chk & 0x1FFFFFF) << 5) ^ v;
    for (std::size_t i = 0; i < kGen.size(); ++i) {
      if ((top >> i) & 1) chk ^= kGen[i];
    }
  };
  for (char c : hrp_text) step(static_cast<unsigned char>(c) >> 5);
  step(0);
  for (char c : hrp_text) step(static_cast<unsigned char>(c) & 31u);
  for (auto v : data) step(v);
  return chk;
}

}  // namespace detail::bech32

class Address {
 public:
  static Address from_account_id(Network network, const AccountId& id) {
    return Address(network, id);
  }

  // An address that must be for `expected`; a valid address for another
  // network is AddressWrongNetwork, so a testnet address never pays mainnet.
  static Result<Address> parse(std::string_view text, Network expected) {
    auto parsed = parse_any_hrp(text);
    if (!parsed) return parsed.error();
    const auto& [hrp_text, id] = parsed.value();
    if (hrp_text != detail::bech32::hrp(expected)) {
      return Error{ErrorCode::AddressWrongNetwork,
                   "address is for network prefix '" + hrp_text + "', but this network expects '" +
                       std::string(detail::bech32::hrp(expected)) + "'"};
    }
    return Address(expected, id);
  }

  // An address for whichever known network its prefix names.
  static Result<Address> parse_any(std::string_view text) {
    auto parsed = parse_any_hrp(text);
    if (!parsed) return parsed.error();
    const auto& [hrp_text, id] = parsed.value();
    for (auto n : {Network::Mainnet, Network::Testnet, Network::Devnet}) {
      if (hrp_text == detail::bech32::hrp(n)) return Address(n, id);
    }
    return Error{ErrorCode::AddressUnknownNetwork,
                 "address prefix '" + hrp_text + "' names no known network"};
  }

  [[nodiscard]] std::string to_string() const {
    using namespace detail::bech32;
    const auto h = hrp(network_);
    std::vector<std::uint8_t> data{kVersion};
    std::uint32_t acc = 0;
    int bits = 0;
    for (auto b : id_) {
      acc = (acc << 8) | b;
      bits += 8;
      while (bits >= 5) {
        bits -= 5;
        data.push_back(static_cast<std::uint8_t>((acc >> bits) & 31));
      }
      acc &= (1u << bits) - 1;
    }
    if (bits > 0) data.push_back(static_cast<std::uint8_t>((acc << (5 - bits)) & 31));

    auto with_zeros = data;
    with_zeros.resize(data.size() + kChecksumLen, 0);
    const std::uint32_t mod = polymod(h, with_zeros) ^ kBech32mConst;
    for (std::size_t i = 0; i < kChecksumLen; ++i) {
      data.push_back(static_cast<std::uint8_t>((mod >> (5 * (kChecksumLen - 1 - i))) & 31));
    }

    std::string out(h);
    out.push_back('1');
    for (auto s : data) out.push_back(kCharset[s]);
    return out;
  }

  [[nodiscard]] Network network() const noexcept { return network_; }
  [[nodiscard]] const AccountId& account_id() const noexcept { return id_; }
  friend bool operator==(const Address&, const Address&) = default;

 private:
  Address(Network network, const AccountId& id) : network_(network), id_(id) {}

  struct Parsed {
    std::string hrp;
    AccountId id;
  };

  static Error err(ErrorCode code, std::string message) { return Error{code, std::move(message)}; }

  // The checks and their order follow address.py `_split` and `_parse`, so
  // every SDK refuses a given string for the same reason.
  static Result<Parsed> parse_any_hrp(std::string_view text) {
    using namespace detail::bech32;
    if (text.size() > kMaxLen) {
      return err(ErrorCode::AddressLength,
                 "address is " + std::to_string(text.size()) + " characters; the limit is 90");
    }
    bool lower = false;
    bool upper = false;
    for (char c : text) {
      lower |= (c >= 'a' && c <= 'z');
      upper |= (c >= 'A' && c <= 'Z');
    }
    if (lower && upper) return err(ErrorCode::AddressMixedCase, "address mixes upper and lower case");
    std::string folded(text);
    for (auto& c : folded) {
      if (c >= 'A' && c <= 'Z') c = static_cast<char>(c - 'A' + 'a');
    }
    const auto sep = folded.rfind('1');
    if (sep == std::string::npos) return err(ErrorCode::AddressNoSeparator, "address has no '1' separator");
    std::string hrp_text = folded.substr(0, sep);
    if (hrp_text.empty()) {
      return err(ErrorCode::AddressEmptyHrp, "address has no network prefix before the '1'");
    }
    for (char ch : hrp_text) {
      const auto c = static_cast<unsigned char>(ch);
      if (c < 33 || c > 126) return err(ErrorCode::AddressInvalidCharacter, "address contains an invalid character");
    }
    const std::string_view body = std::string_view(folded).substr(sep + 1);
    if (body.size() != kDataSymbols + kChecksumLen) {
      return err(ErrorCode::AddressPayload, "address payload is malformed");
    }
    std::vector<std::uint8_t> data;
    data.reserve(body.size());
    for (char c : body) {
      const auto at = kCharset.find(c);
      if (at == std::string_view::npos) {
        return err(ErrorCode::AddressInvalidCharacter, "address contains an invalid character");
      }
      data.push_back(static_cast<std::uint8_t>(at));
    }
    if (polymod(hrp_text, data) != kBech32mConst) {
      return err(ErrorCode::AddressBadChecksum,
                 "address checksum does not match; a character is mistyped");
    }
    if (data[0] != kVersion) {
      return err(ErrorCode::AddressVersion,
                 "address format version " + std::to_string(data[0]) + " is not known");
    }

    // 52 five-bit symbols -> 32 bytes; non-zero padding is non-canonical.
    AccountId id{};
    std::size_t n = 0;
    std::uint32_t acc = 0;
    int bits = 0;
    for (std::size_t i = 1; i < kDataSymbols; ++i) {
      acc = (acc << 5) | data[i];
      bits += 5;
      while (bits >= 8) {
        bits -= 8;
        if (n == id.size()) return err(ErrorCode::AddressPayload, "address payload is malformed");
        id[n++] = static_cast<std::uint8_t>((acc >> bits) & 0xFF);
      }
      acc &= (1u << bits) - 1;
    }
    if (bits >= 5 || acc != 0 || n != id.size()) {
      return err(ErrorCode::AddressPayload, "address payload is malformed");
    }
    return Parsed{std::move(hrp_text), id};
  }

  Network network_;
  AccountId id_;
};

}  // namespace maya2c
