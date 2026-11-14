// Keys through the real sdk-ffi library: round trip, error mapping, handles.
#include <gtest/gtest.h>

#include <array>
#include <cstdint>
#include <type_traits>
#include <utility>
#include <vector>

#include "maya2c/sdk.hpp"

namespace {

using maya2c::ErrorCode;
using maya2c::Network;
using maya2c::Seed;
using maya2c::SigningKey;

static_assert(!std::is_copy_constructible_v<SigningKey>);
static_assert(!std::is_copy_assignable_v<SigningKey>);
static_assert(std::is_nothrow_move_constructible_v<SigningKey>);
static_assert(!std::is_copy_constructible_v<Seed>);
static_assert(!std::is_copy_constructible_v<maya2c::detail::OwnedBuffer>);

const std::vector<std::uint8_t> kSeedSeven(32, 7);
const std::vector<std::uint8_t> kMessage{'m', 'a', 'y', 'a', '2', 'c'};

SigningKey seven() {
  auto key = SigningKey::from_seed(kSeedSeven);
  EXPECT_TRUE(key.has_value());
  return std::move(key).value();
}

TEST(Ffi, ContractVersionMatchesTheBindings) {
  EXPECT_EQ(maya2c::ffi_contract_version(), maya2c::detail::kUniffiContractVersion);
}

TEST(Keys, TheSameSeedDerivesTheSameKey) {
  const auto a = seven();
  const auto b = seven();
  EXPECT_EQ(a.public_key().value(), b.public_key().value());
  EXPECT_EQ(a.account_id().value(), b.account_id().value());
}

TEST(Keys, ATypedSeedDerivesTheSameKeyAsItsBytes) {
  auto seed = Seed::from_bytes(kSeedSeven);
  ASSERT_TRUE(seed.has_value());
  auto key = SigningKey::from_seed(seed.value());
  ASSERT_TRUE(key.has_value());
  EXPECT_EQ(key.value().account_id().value(), seven().account_id().value());
}

TEST(Keys, ASignatureRoundTripsThroughVerify) {
  const auto key = seven();
  auto signature = key.sign(kMessage);
  ASSERT_TRUE(signature.has_value()) << signature.error().message;
  EXPECT_EQ(signature.value().size(), maya2c::kSignatureBytes);
  auto verified = maya2c::verify(key.public_key().value(), kMessage, signature.value());
  EXPECT_TRUE(verified.has_value()) << verified.error().message;
}

TEST(Keys, ATamperedMessageIsVerificationFailed) {
  const auto key = seven();
  const auto signature = key.sign(kMessage).value();
  auto tampered = kMessage;
  tampered.back() = 'd';
  auto verified = maya2c::verify(key.public_key().value(), tampered, signature);
  ASSERT_FALSE(verified.has_value());
  EXPECT_EQ(verified.error().code, ErrorCode::VerificationFailed);
}

TEST(Keys, TheAddressAgreesWithTheStandaloneFunction) {
  const auto key = seven();
  auto from_public = maya2c::account_id_from_public_key(key.public_key().value());
  ASSERT_TRUE(from_public.has_value());
  EXPECT_EQ(from_public.value(), key.account_id().value());

  auto address = key.address(Network::Testnet);
  ASSERT_TRUE(address.has_value());
  auto parsed = maya2c::Address::parse(address.value().to_string(), Network::Testnet);
  ASSERT_TRUE(parsed.has_value());
  EXPECT_EQ(parsed.value().account_id(), key.account_id().value());
}

TEST(Keys, GeneratedKeysDiffer) {
  auto a = SigningKey::generate();
  auto b = SigningKey::generate();
  ASSERT_TRUE(a.has_value());
  ASSERT_TRUE(b.has_value());
  EXPECT_NE(a.value().account_id().value(), b.value().account_id().value());
}

TEST(ErrorMappingLive, AShortSeedIsWrongLengthFromRust) {
  // Passed as raw bytes so the refusal comes from sdk-ffi, not from Seed.
  const std::vector<std::uint8_t> short_seed(31, 0);
  auto key = SigningKey::from_seed(short_seed);
  ASSERT_FALSE(key.has_value());
  EXPECT_EQ(key.error().code, ErrorCode::WrongLength);
  EXPECT_EQ(key.error().expected, 32u);
  EXPECT_EQ(key.error().actual, 31u);
}

TEST(ErrorMappingLive, AMisSizedSignatureIsWrongLength) {
  const auto key = seven();
  auto verified =
      maya2c::verify(key.public_key().value(), kMessage, std::vector<std::uint8_t>(10, 0));
  ASSERT_FALSE(verified.has_value());
  EXPECT_EQ(verified.error().code, ErrorCode::WrongLength);
  EXPECT_EQ(verified.error().expected, static_cast<std::uint32_t>(maya2c::kSignatureBytes));
}

TEST(ErrorMappingLive, AMisSizedPublicKeyIsWrongLength) {
  auto id = maya2c::account_id_from_public_key(std::vector<std::uint8_t>(5, 1));
  ASSERT_FALSE(id.has_value());
  EXPECT_EQ(id.error().code, ErrorCode::WrongLength);
  EXPECT_EQ(id.error().actual, 5u);
}

TEST(Seed, AWrongLengthSeedIsRefusedLocally) {
  auto seed = Seed::from_bytes(std::vector<std::uint8_t>(33, 1));
  ASSERT_FALSE(seed.has_value());
  EXPECT_EQ(seed.error().code, ErrorCode::WrongLength);
}

TEST(Seed, MovingASeedWipesTheSource) {
  auto seed = Seed::from_bytes(kSeedSeven).value();
  Seed moved = std::move(seed);
  for (auto b : seed.expose()) EXPECT_EQ(b, 0);  // NOLINT(bugprone-use-after-move)
  for (auto b : moved.expose()) EXPECT_EQ(b, 7);
}

TEST(SecureZero, ZeroesEveryByte) {
  std::array<std::uint8_t, 64> buf{};
  buf.fill(0xAB);
  maya2c::secure_zero(buf.data(), buf.size());
  for (auto b : buf) EXPECT_EQ(b, 0);
}

TEST(Handles, MoveConstructionTransfersTheHandle) {
  auto a = seven();
  ASSERT_TRUE(a.valid());
  SigningKey b = std::move(a);
  EXPECT_FALSE(a.valid());  // NOLINT(bugprone-use-after-move)
  EXPECT_TRUE(b.valid());
  EXPECT_TRUE(b.sign(kMessage).has_value());
}

TEST(Handles, MoveAssignmentReleasesTheOldKey) {
  auto a = seven();
  auto b = SigningKey::generate().value();
  const auto a_id = a.account_id().value();
  b = std::move(a);
  EXPECT_FALSE(a.valid());  // NOLINT(bugprone-use-after-move)
  EXPECT_EQ(b.account_id().value(), a_id);
}

TEST(Handles, SelfMoveAssignmentKeepsTheKey) {
  auto a = seven();
  auto& alias = a;
  a = std::move(alias);
  EXPECT_TRUE(a.valid());
  EXPECT_TRUE(a.account_id().has_value());
}

TEST(Handles, AMovedFromKeyReportsAnErrorNotUndefinedBehaviour) {
  auto a = seven();
  SigningKey b = std::move(a);
  auto signature = a.sign(kMessage);  // NOLINT(bugprone-use-after-move)
  ASSERT_FALSE(signature.has_value());
  EXPECT_EQ(signature.error().code, ErrorCode::Internal);
  EXPECT_FALSE(a.public_key().has_value());
}

}  // namespace
