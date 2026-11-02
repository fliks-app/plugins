//! The Audible storefronts, which are also the Audnexus `region` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Us,
    Uk,
    Fr,
    De,
    It,
    Es,
    Ca,
    Au,
    Jp,
    In,
}

impl Region {
    pub const ALL: [Region; 10] = [
        Self::Us,
        Self::Uk,
        Self::Fr,
        Self::De,
        Self::It,
        Self::Es,
        Self::Ca,
        Self::Au,
        Self::Jp,
        Self::In,
    ];

    /// Domain suffix of `api.audible.<tld>`.
    pub fn tld(self) -> &'static str {
        match self {
            Self::Us => "com",
            Self::Uk => "co.uk",
            Self::Fr => "fr",
            Self::De => "de",
            Self::It => "it",
            Self::Es => "es",
            Self::Ca => "ca",
            Self::Au => "com.au",
            Self::Jp => "co.jp",
            Self::In => "in",
        }
    }

    /// Two-letter code, also the Audnexus `region` value.
    pub fn code(self) -> &'static str {
        match self {
            Self::Us => "us",
            Self::Uk => "uk",
            Self::Fr => "fr",
            Self::De => "de",
            Self::It => "it",
            Self::Es => "es",
            Self::Ca => "ca",
            Self::Au => "au",
            Self::Jp => "jp",
            Self::In => "in",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|r| r.code() == code.to_lowercase())
    }

    /// The region a metadata language asks for: its country, else the language's home market, else `us`.
    pub fn for_lang(language: &str, country: Option<&str>) -> Self {
        let by_country = country.and_then(|c| match c.to_lowercase().as_str() {
            "gb" => Some(Self::Uk),
            c => Self::from_code(c),
        });
        by_country.unwrap_or(match language.split(['-', '_']).next().unwrap_or("") {
            "fr" => Self::Fr,
            "de" => Self::De,
            "it" => Self::It,
            "es" => Self::Es,
            "ja" => Self::Jp,
            _ => Self::Us,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_metadata_language_picks_a_storefront() {
        assert_eq!(Region::for_lang("fr", Some("CA")), Region::Ca);
        assert_eq!(Region::for_lang("en", Some("GB")), Region::Uk);
        assert_eq!(Region::for_lang("fr-FR", None), Region::Fr);
        assert_eq!(Region::for_lang("ja", Some("")), Region::Jp);
        assert_eq!(Region::for_lang("pt", Some("BR")), Region::Us);
    }
}
