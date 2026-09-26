//! Message catalog: ten locales, one right-to-left (Master Prompt 29 §4).
//!
//! The array length makes a missing string a compile error, not a blank
//! screen. The translations are **unreviewed drafts**: nobody who speaks
//! these languages natively has checked them, and [`REVIEWED`] says so to
//! any UI that wants to show a "beta translation" notice. A native review is
//! a launch gate in `reports/29-interface.md`.

/// Text direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// Left to right.
    Ltr,
    /// Right to left: the layout mirrors, not just the text.
    Rtl,
}

impl Dir {
    /// The HTML `dir` attribute value.
    #[must_use]
    pub const fn attr(self) -> &'static str {
        match self {
            Self::Ltr => "ltr",
            Self::Rtl => "rtl",
        }
    }
}

/// Whether the non-English strings have had a native-speaker review.
pub const REVIEWED: bool = false;

/// Message keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)] // the keys name their English text
pub enum Key {
    Send,
    Receive,
    Swap,
    Review,
    Sign,
    Cancel,
    Retry,
    WarnPoisoning,
    WarnLookalike,
    WarnPhishing,
    ErrorGeneric,
    OfflineBalance,
}

/// Number of keys.
pub const KEYS: usize = 12;

impl Key {
    /// Every key, in catalog order.
    pub const ALL: [Self; KEYS] = [
        Self::Send,
        Self::Receive,
        Self::Swap,
        Self::Review,
        Self::Sign,
        Self::Cancel,
        Self::Retry,
        Self::WarnPoisoning,
        Self::WarnLookalike,
        Self::WarnPhishing,
        Self::ErrorGeneric,
        Self::OfflineBalance,
    ];

    /// Placeholders the string must keep, in every locale.
    #[must_use]
    pub const fn placeholders(self) -> &'static [&'static str] {
        match self {
            Self::WarnPoisoning | Self::WarnLookalike | Self::WarnPhishing => &["{known}"],
            _ => &[],
        }
    }
}

type Row = (&'static str, Dir, [&'static str; KEYS]);

/// The catalog: BCP 47 tag, direction, strings in [`Key::ALL`] order.
pub const CATALOG: &[Row] = &[
    (
        "en",
        Dir::Ltr,
        [
            "Send",
            "Receive",
            "Swap",
            "Review",
            "Sign",
            "Cancel",
            "Try again",
            "This address resembles {known} from your contacts but is different. Check every character.",
            "This token imitates {known}. It is not the verified contract.",
            "This site resembles {known} but is a different address. Do not connect.",
            "Something went wrong. Nothing was sent.",
            "Offline: showing your last known balance.",
        ],
    ),
    (
        "es",
        Dir::Ltr,
        [
            "Enviar",
            "Recibir",
            "Intercambiar",
            "Revisar",
            "Firmar",
            "Cancelar",
            "Reintentar",
            "Esta dirección se parece a {known} de tus contactos, pero es distinta. Revisa cada carácter.",
            "Este token imita a {known}. No es el contrato verificado.",
            "Este sitio se parece a {known}, pero es otra dirección. No te conectes.",
            "Algo salió mal. No se envió nada.",
            "Sin conexión: se muestra tu último saldo conocido.",
        ],
    ),
    (
        "pt-BR",
        Dir::Ltr,
        [
            "Enviar",
            "Receber",
            "Trocar",
            "Revisar",
            "Assinar",
            "Cancelar",
            "Tentar novamente",
            "Este endereço parece com {known} dos seus contatos, mas é diferente. Confira cada caractere.",
            "Este token imita {known}. Não é o contrato verificado.",
            "Este site parece com {known}, mas é outro endereço. Não conecte.",
            "Algo deu errado. Nada foi enviado.",
            "Offline: mostrando seu último saldo conhecido.",
        ],
    ),
    (
        "zh-CN",
        Dir::Ltr,
        [
            "发送",
            "接收",
            "兑换",
            "检查",
            "签名",
            "取消",
            "重试",
            "此地址与联系人中的 {known} 相似，但并不相同。请逐字核对。",
            "此代币仿冒 {known}，不是经过验证的合约。",
            "此网站与 {known} 相似，但地址不同。请勿连接。",
            "出现错误，未发送任何内容。",
            "离线：显示最近一次的余额。",
        ],
    ),
    (
        "hi",
        Dir::Ltr,
        [
            "भेजें",
            "प्राप्त करें",
            "स्वैप",
            "समीक्षा",
            "हस्ताक्षर करें",
            "रद्द करें",
            "फिर से कोशिश करें",
            "यह पता आपके संपर्कों के {known} जैसा दिखता है, पर अलग है। हर अक्षर जाँचें।",
            "यह टोकन {known} की नकल है। यह सत्यापित कॉन्ट्रैक्ट नहीं है।",
            "यह साइट {known} जैसी दिखती है, पर पता अलग है। कनेक्ट न करें।",
            "कुछ गलत हो गया। कुछ भी नहीं भेजा गया।",
            "ऑफ़लाइन: आपका पिछला ज्ञात बैलेंस दिखाया जा रहा है।",
        ],
    ),
    (
        "ru",
        Dir::Ltr,
        [
            "Отправить",
            "Получить",
            "Обменять",
            "Проверить",
            "Подписать",
            "Отмена",
            "Повторить",
            "Этот адрес похож на {known} из ваших контактов, но отличается. Проверьте каждый символ.",
            "Этот токен имитирует {known}. Это не проверенный контракт.",
            "Этот сайт похож на {known}, но это другой адрес. Не подключайтесь.",
            "Что-то пошло не так. Ничего не отправлено.",
            "Нет сети: показан последний известный баланс.",
        ],
    ),
    (
        "ja",
        Dir::Ltr,
        [
            "送金",
            "受け取り",
            "スワップ",
            "確認",
            "署名",
            "キャンセル",
            "再試行",
            "このアドレスは連絡先の {known} に似ていますが、別のものです。一文字ずつ確認してください。",
            "このトークンは {known} を装っています。検証済みのコントラクトではありません。",
            "このサイトは {known} に似ていますが、別のアドレスです。接続しないでください。",
            "問題が発生しました。何も送信されていません。",
            "オフライン：最後に確認した残高を表示しています。",
        ],
    ),
    (
        "ko",
        Dir::Ltr,
        [
            "보내기",
            "받기",
            "스왑",
            "검토",
            "서명",
            "취소",
            "다시 시도",
            "이 주소는 연락처의 {known}와 비슷하지만 다릅니다. 모든 문자를 확인하세요.",
            "이 토큰은 {known}을(를) 사칭합니다. 검증된 컨트랙트가 아닙니다.",
            "이 사이트는 {known}와 비슷하지만 다른 주소입니다. 연결하지 마세요.",
            "문제가 발생했습니다. 아무것도 전송되지 않았습니다.",
            "오프라인: 마지막으로 확인된 잔액을 표시합니다.",
        ],
    ),
    (
        "tr",
        Dir::Ltr,
        [
            "Gönder",
            "Al",
            "Takas",
            "İncele",
            "İmzala",
            "İptal",
            "Tekrar dene",
            "Bu adres kişilerinizdeki {known} adresine benziyor ama farklı. Her karakteri kontrol edin.",
            "Bu token {known} taklidi. Doğrulanmış sözleşme değil.",
            "Bu site {known} sitesine benziyor ama farklı bir adres. Bağlanmayın.",
            "Bir sorun oluştu. Hiçbir şey gönderilmedi.",
            "Çevrimdışı: bilinen son bakiyeniz gösteriliyor.",
        ],
    ),
    (
        "ar",
        Dir::Rtl,
        [
            "إرسال",
            "استلام",
            "مبادلة",
            "مراجعة",
            "توقيع",
            "إلغاء",
            "إعادة المحاولة",
            "هذا العنوان يشبه {known} في جهات اتصالك لكنه مختلف. تحقق من كل حرف.",
            "هذا الرمز يقلّد {known}. إنه ليس العقد الموثّق.",
            "هذا الموقع يشبه {known} لكنه عنوان مختلف. لا تتصل به.",
            "حدث خطأ. لم يُرسَل أي شيء.",
            "غير متصل: يُعرض آخر رصيد معروف لك.",
        ],
    ),
];

/// A string for `locale`, falling back to English for an unknown tag.
#[must_use]
pub fn t(locale: &str, key: Key) -> &'static str {
    let row = CATALOG
        .iter()
        .find(|r| r.0 == locale)
        .unwrap_or(&CATALOG[0]);
    row.2[key as usize]
}

/// Text direction for `locale` (English's for an unknown tag).
#[must_use]
pub fn dir(locale: &str) -> Dir {
    CATALOG
        .iter()
        .find(|r| r.0 == locale)
        .map_or(Dir::Ltr, |r| r.1)
}
