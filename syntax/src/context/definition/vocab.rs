use crate::{CompactIri, ExpandableRef};
use iref::Iri;
use locspan_derive::StrippedPartialEq;
use rdf_types::BlankId;

#[derive(Clone, PartialEq, StrippedPartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Vocab(#[locspan(stripped)] String);

impl Vocab {
	pub fn as_iri(&self) -> Option<Iri> {
		Iri::new(&self.0).ok()
	}

	pub fn as_compact_iri(&self) -> Option<&CompactIri> {
		CompactIri::new(&self.0).ok()
	}

	pub fn as_blank_id(&self) -> Option<&BlankId> {
		BlankId::new(&self.0).ok()
	}

	pub fn as_str(&self) -> &str {
		&self.0
	}

	pub fn into_string(self) -> String {
		self.0
	}
}

impl From<String> for Vocab {
	fn from(s: String) -> Self {
		Self(s)
	}
}

#[derive(Clone, Copy)]
pub struct VocabRef<'a>(&'a str);

impl<'a> VocabRef<'a> {
	pub fn as_str(&self) -> &'a str {
		self.0
	}
}

impl<'a> From<&'a Vocab> for VocabRef<'a> {
	fn from(v: &'a Vocab) -> Self {
		Self(v.as_str())
	}
}

impl<'a> From<VocabRef<'a>> for ExpandableRef<'a> {
	fn from(v: VocabRef<'a>) -> Self {
		ExpandableRef::String(v.0)
	}
}
