// Copyright 2019 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

use proc_macro2::Span;
use srml_support_procedural_tools::{syn_ext as ext, Parse, ToTokens};
use std::collections::{BTreeMap, HashMap, HashSet};
use syn::{
	parse::{Parse, ParseStream},
	spanned::Spanned,
	Ident, Result, Token,
};

mod keyword {
	syn::custom_keyword!(Block);
	syn::custom_keyword!(NodeBlock);
	syn::custom_keyword!(UncheckedExtrinsic);
}

#[derive(Debug)]
pub struct RuntimeDefinition {
	pub visibility_token: Token![pub],
	pub enum_token: Token![enum],
	pub name: Ident,
	pub where_section: WhereSection,
	pub modules: ext::Braces<ext::Punctuated<ModuleDeclaration, Token![,]>>,
}

impl Parse for RuntimeDefinition {
	fn parse(input: ParseStream) -> Result<Self> {
		Ok(
			Self {
				visibility_token: input.parse()?,
				enum_token: input.parse()?,
				name: input.parse()?,
				where_section: input.parse()?,
				modules: input.parse()?,
			}
		)
	}
}

#[derive(Debug)]
pub struct WhereSection {
	pub block: syn::TypePath,
	pub node_block: syn::TypePath,
	pub unchecked_extrinsic: syn::TypePath,
}

impl Parse for WhereSection {
	fn parse(input: ParseStream) -> Result<Self> {
		let _: Token![where] = input.parse()?;
		let mut seen_keys = HashSet::new();
		let mut definitions = HashMap::new();
		loop {
			let WhereDefinition { kind, value } = WhereDefinition::parse(input, &mut seen_keys)?;
			definitions.insert(kind, value);
			if !input.peek(Token![,]) { break; }
			let _: Token![,] = input.parse()?;
		}
		let expected_seen_keys: HashSet<_> = vec![WhereKind::Block, WhereKind::NodeBlock, WhereKind::UncheckedExtrinsic].into_iter().collect();
		let diff: Vec<_> = expected_seen_keys.difference(&seen_keys).collect();
		if diff.len() > 0 {
			let msg = format!("Missing associated type for `{:?}`. Add `{:?}` = ... to where section", diff[0], diff[0]);
			return Err(input.error(msg))
		}
		Ok(
			Self {
				block: definitions.remove(&WhereKind::Block).expect("Definitions is guaranteed to have this key; qed"),
				node_block: definitions.remove(&WhereKind::NodeBlock).expect("Definitions is guaranteed to have this key; qed"),
				unchecked_extrinsic: definitions.remove(&WhereKind::UncheckedExtrinsic).expect("Definitions is guaranteed to have this key; qed"),
			}
		)
	}
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub enum WhereKind {
	Block,
	NodeBlock,
	UncheckedExtrinsic,
}

impl WhereKind {
	pub fn parse(input: ParseStream, seen_keys: &mut HashSet<WhereKind>) -> Result<Self> {
		let lookahead = input.lookahead1();
        if lookahead.peek(keyword::Block) {
			WhereKind::Block.parse_concrete::<keyword::Block>(input, seen_keys)
        } else if lookahead.peek(keyword::NodeBlock) {
			WhereKind::NodeBlock.parse_concrete::<keyword::NodeBlock>(input, seen_keys)
        } else if lookahead.peek(keyword::UncheckedExtrinsic) {
			WhereKind::UncheckedExtrinsic.parse_concrete::<keyword::UncheckedExtrinsic>(input, seen_keys)
        } else {
            Err(lookahead.error())
        }
	}

	fn parse_concrete<KW: Parse>(&self, input: ParseStream, seen_keys: &mut HashSet<WhereKind>) -> Result<Self> {
		if seen_keys.contains(self) {
			let msg = format!("`{:?}` was declared above. Please use exactly one delcataion for `{:?}`", self, self);
			return Err(input.error(msg))
		}
		seen_keys.insert(*self);
		let _: KW = input.parse()?;
		Ok(*self)
	}
}

#[derive(Debug)]
pub struct WhereDefinition {
	pub kind: WhereKind,
	pub value: syn::TypePath,
}

impl WhereDefinition {
	fn parse(input: ParseStream, seen_keys: &mut HashSet<WhereKind>) -> Result<Self> {
		Ok(
			Self {
				kind: WhereKind::parse(input, seen_keys)?,
				value: {
					let _: Token![=] = input.parse()?;
					input.parse()?
				}
			}
		)
	}
}

#[derive(Parse, ToTokens, Debug)]
pub struct ModuleDeclaration {
	pub name: Ident,
	pub name_colon: Token![:],
	pub module: Ident,
	pub instance: ModuleInstanceWrapper,
	pub details: ModuleDetailsWrapper,
}

#[derive(ToTokens, Debug)]
pub struct ModuleInstanceWrapper {
	pub inner: Option<ModuleInstance>,
}

impl Parse for ModuleInstanceWrapper {
	fn parse(input: ParseStream) -> Result<Self> {
		// In this case we're sure it needs to be a ModuleInstance
		if input.peek(Token![::]) && input.peek3(Token![<]) {
			let inner = Some(input.parse()?);
			Ok(ModuleInstanceWrapper { inner })
		} else {
			Ok(ModuleInstanceWrapper { inner: None })
		}
	}
}

#[derive(ToTokens, Debug)]
pub struct ModuleDetailsWrapper {
	pub inner: Option<ModuleDetails>,
}

impl Parse for ModuleDetailsWrapper {
	fn parse(input: ParseStream) -> Result<Self> {
		// In this case we're sure it needs to be a ModuleDetails
		if input.peek(Token![::]) {
			let inner = Some(input.parse()?);
			Ok(ModuleDetailsWrapper { inner })
		} else {
			Ok(ModuleDetailsWrapper { inner: None })
		}
	}
}

#[derive(Parse, ToTokens, Debug)]
pub struct ModuleInstance {
	pub colons: Token![::],
	pub lt: Token![<],
	pub name: Ident,
	pub gt: Token![>],
}

#[derive(Parse, ToTokens, Debug)]
pub struct ModuleDetails {
	pub colons: Token![::],
	pub entries: ext::Braces<ext::Punctuated<ModuleEntryWrapper, Token![,]>>,
}

#[derive(ToTokens, Debug)]
pub struct ModuleEntryWrapper {
	pub inner: ModuleEntry,
}

impl Parse for ModuleEntryWrapper {
	fn parse(input: ParseStream) -> Result<Self> {
		ModuleEntry::parse(input)
			.map(|inner| ModuleEntryWrapper { inner })
			.map_err(|_| input.error("Expected `default` or module export name (e.g. Call, Event, etc.)"))
	}
}

#[derive(Parse, ToTokens, Debug)]
pub enum ModuleEntry {
	Default(Token![default]),
	Part(ModulePart),
}

#[derive(Parse, ToTokens, Debug, Clone)]
pub struct ModulePart {
	pub name: Ident,
	pub generics: syn::Generics,
	pub args: ext::Opt<ext::Parens<ext::Punctuated<Ident, Token![,]>>>,
}

impl ModuleDeclaration {
	/// Get resolved module parts, i.e. after expanding `default` keyword
	/// or empty declaration
	pub fn module_parts(&self) -> Vec<ModulePart> {
		if let Some(ref details) = self.details.inner {
			let uniq: BTreeMap<_, _> = details
				.entries
				.content
				.inner
				.iter()
				.flat_map(|entry| match &entry.inner {
					ModuleEntry::Default(ref token) => Self::default_modules(token.span()),
					ModuleEntry::Part(ref part) => vec![part.clone()],
				})
				.map(|part| (part.name.to_string(), part))
				.collect();
			uniq.into_iter().map(|(_, v)| v).collect()
		} else {
			Self::default_modules(self.module.span())
		}
	}

	fn default_modules(span: Span) -> Vec<ModulePart> {
		let mut res: Vec<_> = ["Module", "Call", "Storage"]
			.into_iter()
			.map(|name| ModulePart::with_name(name, span))
			.collect();
		res.extend(
			["Event", "Config"]
				.into_iter()
				.map(|name| ModulePart::with_generics(name, span)),
		);
		res
	}
}

impl ModulePart {
	/// Plain module name like `Event` or `Call`, etc.
	pub fn with_name(name: &str, span: Span) -> Self {
		let name = Ident::new(name, span);
		Self {
			name,
			generics: syn::Generics {
				lt_token: None,
				gt_token: None,
				where_clause: None,
				..Default::default()
			},
			args: ext::Opt { inner: None },
		}
	}

	/// Module name with generic like `Event<T>` or `Call<T>`, etc.
	pub fn with_generics(name: &str, span: Span) -> Self {
		let name = Ident::new(name, span);
		let typ = Ident::new("T", span);
		let generic_param = syn::GenericParam::Type(typ.into());
		let generic_params = vec![generic_param].into_iter().collect();
		let generics = syn::Generics {
			lt_token: Some(syn::token::Lt { spans: [span] }),
			params: generic_params,
			gt_token: Some(syn::token::Gt { spans: [span] }),
			where_clause: None,
		};
		Self {
			name,
			generics,
			args: ext::Opt { inner: None },
		}
	}
}
