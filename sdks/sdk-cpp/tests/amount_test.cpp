// Amount parsing and formatting: exact 256-bit integers, exactly 24 decimals.
#include <gtest/gtest.h>

#include <string>

#include "maya2c/amount.hpp"

namespace {

using maya2c::Amount;
using maya2c::ErrorCode;

// 2^256 - 1, the largest base-unit amount (crates/amount `Amount::MAX`).
constexpr const char* kMaxBaseUnits =
    "115792089237316195423570985008687907853269984665640564039457584007913129639935";

std::string max_as_coins() {
  const std::string base = kMaxBaseUnits;
  return base.substr(0, base.size() - Amount::kDecimals) + "." +
         base.substr(base.size() - Amount::kDecimals);
}

TEST(Amount, ZeroFormatsWithAllTwentyFourDecimals) {
  const Amount zero;
  EXPECT_TRUE(zero.is_zero());
  EXPECT_EQ(zero.to_base_units(), "0");
  EXPECT_EQ(zero.to_coins(), "0.000000000000000000000000");
}

TEST(Amount, ZeroParsesInEveryAcceptedSpelling) {
  for (const char* text : {"0", "0.0", "0.000000000000000000000000"}) {
    auto parsed = Amount::parse_coins(text);
    ASSERT_TRUE(parsed.has_value()) << text;
    EXPECT_TRUE(parsed.value().is_zero()) << text;
  }
}

TEST(Amount, OneCoinIsTenToTheTwentyFourBaseUnits) {
  auto one = Amount::parse_coins("1");
  ASSERT_TRUE(one.has_value());
  EXPECT_EQ(one.value().to_base_units(), "1000000000000000000000000");
  EXPECT_EQ(one.value().to_coins(), "1.000000000000000000000000");
}

TEST(Amount, OneBaseUnitIsTheSmallestFraction) {
  auto unit = Amount::parse_coins("0.000000000000000000000001");
  ASSERT_TRUE(unit.has_value());
  EXPECT_EQ(unit.value(), Amount::from_u64(1));
  EXPECT_EQ(Amount::from_u64(1).to_coins(), "0.000000000000000000000001");
}

TEST(Amount, AFractionIsScaledNotTruncated) {
  auto half = Amount::parse_coins("1.5");
  ASSERT_TRUE(half.has_value());
  EXPECT_EQ(half.value().to_base_units(), "1500000000000000000000000");
}

TEST(Amount, MaxRoundTripsInBaseUnitsAndCoins) {
  auto max = Amount::from_base_units(kMaxBaseUnits);
  ASSERT_TRUE(max.has_value());
  EXPECT_EQ(max.value(), Amount::max());
  EXPECT_EQ(max.value().to_base_units(), kMaxBaseUnits);
  EXPECT_EQ(max.value().to_coins(), max_as_coins());

  auto back = Amount::parse_coins(max_as_coins());
  ASSERT_TRUE(back.has_value());
  EXPECT_EQ(back.value(), Amount::max());
}

TEST(Amount, OneAboveMaxIsRefusedAsOverflow) {
  auto over = Amount::from_base_units(
      "115792089237316195423570985008687907853269984665640564039457584007913129639936");
  ASSERT_FALSE(over.has_value());
  EXPECT_EQ(over.error().code, ErrorCode::AmountOverflow);
}

TEST(Amount, MaxPlusOneBaseUnitInCoinsIsRefusedAsOverflow) {
  // The same 2^256 written in coins: the scaling multiply must catch it.
  const std::string base =
      "115792089237316195423570985008687907853269984665640564039457584007913129639936";
  const std::string coins = base.substr(0, base.size() - 24) + "." + base.substr(base.size() - 24);
  auto over = Amount::parse_coins(coins);
  ASSERT_FALSE(over.has_value());
  EXPECT_EQ(over.error().code, ErrorCode::AmountOverflow);
}

TEST(Amount, ATwentyFifthDecimalIsRefusedRatherThanRounded) {
  // Silently rounding a payment is a loss somebody did not agree to.
  for (const char* text : {"0.0000000000000000000000001", "1.0000000000000000000000000",
                           "1.0000000000000000000000005"}) {
    auto parsed = Amount::parse_coins(text);
    ASSERT_FALSE(parsed.has_value()) << text;
    EXPECT_EQ(parsed.error().code, ErrorCode::AmountTooPrecise) << text;
  }
}

TEST(Amount, MalformedTextIsRefused) {
  for (const char* text : {"", ".", "1.", ".5", "-1", "+1", " 1", "1 ", "1e3", "1,5", "1.2.3",
                           "0x10", "\xef\xbc\x91" /* fullwidth one */}) {
    auto parsed = Amount::parse_coins(text);
    ASSERT_FALSE(parsed.has_value()) << '"' << text << '"';
    EXPECT_EQ(parsed.error().code, ErrorCode::AmountSyntax) << '"' << text << '"';
  }
}

TEST(Amount, BaseUnitsRefuseAFractionPoint) {
  auto parsed = Amount::from_base_units("1.5");
  ASSERT_FALSE(parsed.has_value());
  EXPECT_EQ(parsed.error().code, ErrorCode::AmountSyntax);
}

TEST(Amount, LittleEndianBytesMatchTheChainEncoding) {
  // crates/amount `to_le_bytes`: least significant byte first.
  const auto bytes = Amount::from_u64(0x0102).to_le_bytes();
  EXPECT_EQ(bytes[0], 0x02);
  EXPECT_EQ(bytes[1], 0x01);
  for (std::size_t i = 2; i < bytes.size(); ++i) {
    EXPECT_EQ(bytes[i], 0) << i;
  }
  EXPECT_EQ(Amount::from_le_bytes(Amount::max().to_le_bytes()), Amount::max());
}

TEST(Amount, CheckedArithmeticRefusesWrapAround) {
  EXPECT_FALSE(Amount::max().checked_add(Amount::from_u64(1)).has_value());
  EXPECT_FALSE(Amount().checked_sub(Amount::from_u64(1)).has_value());
  auto sum = Amount::from_u64(0xFFFFFFFFu).checked_add(Amount::from_u64(1));
  ASSERT_TRUE(sum.has_value());
  EXPECT_EQ(sum->to_base_units(), "4294967296");
  EXPECT_LT(Amount::from_u64(1), Amount::max());
}

}  // namespace
