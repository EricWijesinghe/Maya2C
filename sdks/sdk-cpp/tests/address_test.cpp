// Bech32m addresses (ADR-081), checked against spec/address-vectors.json.
#include <gtest/gtest.h>

#include <array>
#include <cstdint>
#include <string>

#include "maya2c/address.hpp"

namespace {

using maya2c::AccountId;
using maya2c::Address;
using maya2c::ErrorCode;
using maya2c::Network;

AccountId id_from_hex(const std::string& hex) {
  AccountId id{};
  for (std::size_t i = 0; i < id.size(); ++i) {
    id[i] = static_cast<std::uint8_t>(std::stoul(hex.substr(2 * i, 2), nullptr, 16));
  }
  return id;
}

struct Vector {
  Network network;
  const char* id_hex;
  const char* text;
};

// Copied from spec/address-vectors.json `valid`, which an independent BIP-350
// reference generated. Every SDK reads the same file.
constexpr std::array<Vector, 6> kValid{{
    {Network::Mainnet, "0000000000000000000000000000000000000000000000000000000000000000",
     "maya1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqf79v63"},
    {Network::Mainnet, "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
     "maya1qlllllllllllllllllllllllllllllllllllllllllllllllllllstqxanx"},
    {Network::Mainnet, "06a9e5fc36d639816b77a9ab6619575df857f67503a8e33d05256a124e47d7cd",
     "maya1qq657tlpk6cucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lxsgd736f"},
    {Network::Testnet, "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
     "tmaya1qqqqsyqcyq5rqwzqfpg9scrgwpugpzysnzs23v9ccrydpk8qarc0slt6dar"},
    {Network::Testnet, "06a9e5fc36d639816b77a9ab6619575df857f67503a8e33d05256a124e47d7cd",
     "tmaya1qq657tlpk6cucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lxshv2jaz"},
    {Network::Devnet, "06a9e5fc36d639816b77a9ab6619575df857f67503a8e33d05256a124e47d7cd",
     "dmaya1qq657tlpk6cucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lxsg9wep2"},
}};

TEST(Address, SharedVectorsEncode) {
  for (const auto& v : kValid) {
    EXPECT_EQ(Address::from_account_id(v.network, id_from_hex(v.id_hex)).to_string(), v.text);
  }
}

TEST(Address, SharedVectorsDecode) {
  for (const auto& v : kValid) {
    auto parsed = Address::parse(v.text, v.network);
    ASSERT_TRUE(parsed.has_value()) << v.text;
    EXPECT_EQ(parsed.value().account_id(), id_from_hex(v.id_hex));
    EXPECT_EQ(parsed.value().network(), v.network);
  }
}

TEST(Address, UpperCaseIsAccepted) {
  auto parsed =
      Address::parse("MAYA1QQ657TLPK6CUCZ6MH4X4KVX2HTHU90AN4QW5WX0G9Y44PYNJ86LXSGD736F",
                     Network::Mainnet);
  ASSERT_TRUE(parsed.has_value());
  EXPECT_EQ(parsed.value().to_string(),
            "maya1qq657tlpk6cucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lxsgd736f");
}

struct Refusal {
  const char* name;
  const char* text;
  Network expected;
  ErrorCode code;
};

// spec/address-vectors.json `refused`, minus unknown-prefix (tested below,
// because it has no expected network).
constexpr std::array<Refusal, 7> kRefused{{
    {"wrong-network", "tmaya1qq657tlpk6cucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lxshv2jaz",
     Network::Mainnet, ErrorCode::AddressWrongNetwork},
    {"typo-in-payload", "tmaya1qq657tlpk6qucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lxshv2jaz",
     Network::Testnet, ErrorCode::AddressBadChecksum},
    {"typo-in-checksum", "tmaya1qq657tlpk6cucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lxshv2jaq",
     Network::Testnet, ErrorCode::AddressBadChecksum},
    {"mixed-case", "Tmaya1qq657tlpk6cucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lxshv2jaz",
     Network::Testnet, ErrorCode::AddressMixedCase},
    {"nonzero-padding", "tmaya1qq657tlpk6cucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lx32678qs",
     Network::Testnet, ErrorCode::AddressPayload},
    {"unknown-version", "tmaya1pq657tlpk6cucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lxsg86hqu",
     Network::Testnet, ErrorCode::AddressVersion},
    {"no-separator", "qyrz8wqd2c9m", Network::Testnet, ErrorCode::AddressNoSeparator},
}};

TEST(Address, SharedRefusalsAreRefusedForTheStatedReason) {
  for (const auto& r : kRefused) {
    auto parsed = Address::parse(r.text, r.expected);
    ASSERT_FALSE(parsed.has_value()) << r.name;
    EXPECT_EQ(parsed.error().code, r.code) << r.name << ": " << parsed.error().message;
  }
}

TEST(Address, AnUnknownPrefixNamesNoNetwork) {
  auto parsed =
      Address::parse_any("btc1qq657tlpk6cucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lxs4nvruj");
  ASSERT_FALSE(parsed.has_value());
  EXPECT_EQ(parsed.error().code, ErrorCode::AddressUnknownNetwork);
}

TEST(Address, ParseAnyFindsTheNetwork) {
  auto parsed =
      Address::parse_any("dmaya1qq657tlpk6cucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lxsg9wep2");
  ASSERT_TRUE(parsed.has_value());
  EXPECT_EQ(parsed.value().network(), Network::Devnet);
}

TEST(Address, StructuralRefusals) {
  EXPECT_EQ(Address::parse(std::string(91, 'q'), Network::Mainnet).error().code,
            ErrorCode::AddressLength);
  EXPECT_EQ(Address::parse("1qqqqqq", Network::Mainnet).error().code, ErrorCode::AddressEmptyHrp);
  EXPECT_EQ(Address::parse("maya1qqqb", Network::Mainnet).error().code, ErrorCode::AddressPayload);
  // 'b' is not in the Bech32 alphabet; the body length is right so the
  // character check, not the length check, is what refuses it.
  EXPECT_EQ(Address::parse("maya1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqf79v6b",
                           Network::Mainnet)
                .error()
                .code,
            ErrorCode::AddressInvalidCharacter);
  EXPECT_EQ(Address::parse(std::string("ma\x7fya1q"), Network::Mainnet).error().code,
            ErrorCode::AddressInvalidCharacter);
}

}  // namespace
