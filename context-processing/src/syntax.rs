use json_ld_core::{
	Id,
	Context,
	ProcessingMode,
	Term
};
use json_ld_syntax::{
	self as syntax,
	Nullable
};
use iref::{Iri, IriBuf, IriRef};
use futures::future::{BoxFuture, FutureExt};
use locspan::{Loc, At};
use crate::{
	Process,
	ProcessingStack,
	ProcessingOptions,
	ProcessingResult,
	ProcessedContext,
	Loader,
	LocWarning,
	Error,
	LocError
};

mod iri;
mod define;
mod merged;

use iri::*;
use define::*;
use merged::*;

impl<C: syntax::AnyContextEntry, T: Id> Process<T> for C {
	type Source = C::Source;
	type Span = C::Span;
	
	fn process_full<'a, L: Loader + Send + Sync>(
		&'a self,
		active_context: &'a Context<T, C>,
		stack: ProcessingStack,
		loader: &'a mut L,
		base_url: Option<Iri<'a>>,
		options: ProcessingOptions,
	) -> BoxFuture<'a, ProcessingResult<T, C>>
	where
		L::Output: Into<C>,
		T: Send + Sync
	{
		async move {
			let mut warnings = Vec::new();
			let processed = process_context(
				active_context,
				self,
				stack,
				loader,
				base_url,
				options,
				&mut warnings
			).await?;

			Ok(ProcessedContext::with_warnings(processed, warnings))
		}.boxed()
	}
}

/// Resolve `iri_ref` against the given base IRI.
fn resolve_iri(iri_ref: IriRef, base_iri: Option<Iri>) -> Option<IriBuf> {
	match base_iri {
		Some(base_iri) => Some(iri_ref.resolved(base_iri)),
		None => match iri_ref.into_iri() {
			Ok(iri) => Some(iri.into()),
			Err(_) => None,
		},
	}
}

// This function tries to follow the recommended context processing algorithm.
// See `https://www.w3.org/TR/json-ld11-api/#context-processing-algorithm`.
//
// The recommended default value for `remote_contexts` is the empty set,
// `false` for `override_protected`, and `true` for `propagate`.
fn process_context<'a, T, C, L>(
	active_context: &'a Context<T, C>,
	local_context: &'a C,
	mut remote_contexts: ProcessingStack,
	loader: &'a mut L,
	base_url: Option<Iri>,
	mut options: ProcessingOptions,
	warnings: &'a mut Vec<LocWarning<T, C>>,
) -> BoxFuture<'a, Result<Context<T, C>, LocError<T, C>>>
where
	T: Id + Send + Sync,
	C: Clone + syntax::AnyContextEntry + Process<T, Source=<C as syntax::AnyContextEntry>::Source, Span=<C as syntax::AnyContextEntry>::Span>,
	L: Loader + Send + Sync,
	L::Output: Into<C>
{
	use syntax::AnyContextDefinition;
	let base_url_buf = base_url.map(IriBuf::from);

	async move {
		let base_url = base_url_buf.as_ref().map(|base_url| base_url.as_iri());

		// 1) Initialize result to the result of cloning active context.
		let mut result = active_context.clone();

		// 2) If `local_context` is an object containing the member @propagate,
		// its value MUST be boolean true or false, set `propagate` to that value.
		let local_context_ref = local_context.as_entry_ref();
		if let syntax::ContextEntryRef::One(Loc(syntax::ContextRef::Definition(def), _)) = local_context_ref {
			if let Some(propagate) = def.propagate() {
				options.propagate = *propagate.value()
			}
		}

		// 3) If propagate is false, and result does not have a previous context,
		// set previous context in result to active context.
		if !options.propagate && result.previous_context().is_none() {
			result.set_previous_context(active_context.clone());
		}

		// 4) If local context is not an array, set it to an array containing only local context.
		// 5) For each item context in local context:
		for Loc(context, context_loc) in local_context_ref {
			match context {
				// 5.1) If context is null:
				syntax::ContextRef::Null => {
					// If `override_protected` is false and `active_context` contains any protected term
					// definitions, an invalid context nullification has been detected and processing
					// is aborted.
					if !options.override_protected && result.has_protected_items() {
						let e: LocError<T, C> = Error::InvalidContextNullification
							.at(context_loc);
						return Err(e);
					} else {
						// Otherwise, initialize result as a newly-initialized active context, setting
						// previous_context in result to the previous value of result if propagate is
						// false. Continue with the next context.
						let previous_result = result;

						// Initialize `result` as a newly-initialized active context, setting both
						// `base_iri` and `original_base_url` to the value of `original_base_url` in
						// active context, ...
						result = Context::new(active_context.original_base_url());

						// ... and, if `propagate` is `false`, `previous_context` in `result` to the
						// previous value of `result`.
						if !options.propagate {
							result.set_previous_context(previous_result);
						}
					}
				}

				// 5.2) If context is a string,
				syntax::ContextRef::IriRef(iri_ref) => {
					// Initialize `context` to the result of resolving context against base URL.
					// If base URL is not a valid IRI, then context MUST be a valid IRI, otherwise
					// a loading document failed error has been detected and processing is aborted.
					let context_iri = resolve_iri(iri_ref, base_url).ok_or_else(|| {
						Error::LoadingDocumentFailed
							.at(context_loc.clone())
					})?;

					// If the number of entries in the `remote_contexts` array exceeds a processor
					// defined limit, a context overflow error has been detected and processing is
					// aborted; otherwise, add context to remote contexts.
					//
					// If context was previously dereferenced, then the processor MUST NOT do a further
					// dereference, and context is set to the previously established internal
					// representation: set `context_document` to the previously dereferenced document,
					// and set loaded context to the value of the @context entry from the document in
					// context document.
					//
					// Otherwise, set `context document` to the RemoteDocument obtained by dereferencing
					// context using the LoadDocumentCallback, passing context for url, and
					// http://www.w3.org/ns/json-ld#context for profile and for requestProfile.
					//
					// If context cannot be dereferenced, or the document from context document cannot
					// be transformed into the internal representation , a loading remote context
					// failed error has been detected and processing is aborted.
					// If the document has no top-level map with an @context entry, an invalid remote
					// context has been detected and processing is aborted.
					// Set loaded context to the value of that entry.
					if remote_contexts.push(context_iri.as_iri()) {
						let loaded_context = loader
							.load_context(context_iri.as_iri())
							.await
							.map_err(|e| e.at(context_loc))?.into();

						// Set result to the result of recursively calling this algorithm, passing result
						// for active context, loaded context for local context, the documentUrl of context
						// document for base URL, and a copy of remote contexts.
						let new_options = ProcessingOptions {
							processing_mode: options.processing_mode,
							override_protected: false,
							propagate: true,
						};

						let (processed, processed_warnings) = loaded_context
							.process_full(
								&result,
								remote_contexts.clone(),
								loader,
								Some(context_iri.as_iri()),
								new_options,
							)
							.await?
							.into_parts();

						warnings.extend(processed_warnings);
						result = processed
					}
				}

				// 5.4) Context definition.
				syntax::ContextRef::Definition(context) => {
					// 5.5) If context has a @version entry:
					if let Some(version_value) = context.version() {
						// 5.5.2) If processing mode is set to json-ld-1.0, a processing mode conflict
						// error has been detected.
						if options.processing_mode == ProcessingMode::JsonLd1_0 {
							return Err(Error::ProcessingModeConflict
								.at(version_value.location().clone().cast()));
						}
					}

					// 5.6) If context has an @import entry:
					let context: Merged<'a, C> = if let Some(Loc(import_value, import_loc)) = context.import() {
						// 5.6.1) If processing mode is json-ld-1.0, an invalid context entry error
						// has been detected.
						if options.processing_mode == ProcessingMode::JsonLd1_0 {
							return Err(Error::InvalidContextEntry
								.at(import_loc));
						}

						// 5.6.3) Initialize import to the result of resolving the value of
						// @import.
						let import = resolve_iri(import_value, base_url).ok_or_else(|| {
							Error::InvalidImportValue
								.at(import_loc.clone())
						})?;

						// 5.6.4) Dereference import.
						let import_context: C = loader
							.load_context(import.as_iri())
							.await
							.map_err(|e| e.at(import_loc.clone()))?.into();

						// If the dereferenced document has no top-level map with an @context
						// entry, or if the value of @context is not a context definition
						// (i.e., it is not an map), an invalid remote context has been
						// detected and processing is aborted; otherwise, set import context
						// to the value of that entry.
						match import_context.as_entry_ref() {
							syntax::ContextEntryRef::One(Loc(syntax::ContextRef::Definition(import_context_def), _)) => {
								// If `import_context` has a @import entry, an invalid context entry
								// error has been detected and processing is aborted.
								if let Some(Loc(_, loc)) = import_context_def.import() {
									return Err(Error::InvalidContextEntry.at(loc));
								}
							}
							_ => {
								return Err(Error::InvalidRemoteContext
									.at(import_loc));
							}
						}

						// Set `context` to the result of merging context into
						// `import_context`, replacing common entries with those from
						// `context`.
						Merged::new(context, Some(import_context))
					} else {
						Merged::new(context, None)
					};

					// 5.7) If context has a @base entry and remote contexts is empty, i.e.,
					// the currently being processed context is not a remote context:
					if remote_contexts.is_empty() {
						// Initialize value to the value associated with the @base entry.
						if let Some(Loc(value, base_loc)) = context.base() {
							match value {
								syntax::Nullable::Null => {
									// If value is null, remove the base IRI of result.
									result.set_base_iri(None);
								}
								syntax::Nullable::Some(iri_ref) => {
									match iri_ref.into_iri() {
										Ok(iri) => result.set_base_iri(Some(iri)),
										Err(not_iri) => {
											let resolved =
												resolve_iri(not_iri, result.base_iri())
													.ok_or_else(|| {
													Error::InvalidBaseIri.at(base_loc)
												})?;
											result.set_base_iri(Some(resolved.as_iri()))
										}
									}
								}
							}
						}
					}

					// 5.8) If context has a @vocab entry:
					// Initialize value to the value associated with the @vocab entry.
					if let Some(Loc(value, vocab_loc)) = context.vocab() {
						match value {
							syntax::Nullable::Null => {
								// If value is null, remove any vocabulary mapping from result.
								result.set_vocabulary(None);
							}
							syntax::Nullable::Some(value) => {
								// Otherwise, if value is an IRI or blank node identifier, the
								// vocabulary mapping of result is set to the result of IRI
								// expanding value using true for document relative. If it is not
								// an IRI, or a blank node identifier, an invalid vocab mapping
								// error has been detected and processing is aborted.
								// NOTE: The use of blank node identifiers to value for @vocab is
								// obsolete, and may be removed in a future version of JSON-LD.
								match expand_iri_simple(
									&result,
									Loc(Nullable::Some(value.into()), vocab_loc.clone()),
									true,
									true,
									warnings,
								) {
									Term::Ref(vocab) => {
										result.set_vocabulary(Some(Term::Ref(vocab)))
									}
									_ => {
										return Err(Error::InvalidVocabMapping
											.at(vocab_loc))
									}
								}
							}
						}
					}

					// 5.9) If context has a @language entry:
					if let Some(Loc(value, _language_loc)) = context.language() {
						match value {
							Nullable::Null => {
								// 5.9.2) If value is null, remove any default language from result.
								result.set_default_language(None);
							}
							Nullable::Some(tag) => {
								result.set_default_language(Some(tag.to_owned()));
							}
						}
					}

					// 5.10) If context has a @direction entry:
					if let Some(Loc(value, direction_loc)) = context.direction() {
						// 5.10.1) If processing mode is json-ld-1.0, an invalid context entry error
						// has been detected and processing is aborted.
						if options.processing_mode == ProcessingMode::JsonLd1_0 {
							return Err(Error::InvalidContextEntry
								.at(direction_loc));
						}

						match value {
							Nullable::Null => {
								// 5.10.3) If value is null, remove any base direction from result.
								result.set_default_base_direction(None);
							}
							Nullable::Some(dir) => {
								result.set_default_base_direction(Some(dir));
							}
						}
					}

					// 5.12) Create a map `defined` to keep track of whether or not a term
					// has already been defined or is currently being defined during recursion.
					let mut defined = DefinedTerms::new();
					let protected = context.protected().map(Loc::into_value).unwrap_or(false);

					// 5.13) For each key-value pair in context where key is not
					// @base, @direction, @import, @language, @propagate, @protected, @version,
					// or @vocab,
					// invoke the Create Term Definition algorithm passing result for
					// active context, context for local context, key, defined, base URL,
					// and the value of the @protected entry from context, if any, for protected.
					// (and the value of override protected)
					for (key, binding) in context.bindings() {
						define(
							&mut result,
							&context,
							Loc(key.into(), binding.key_location().clone()),
							&mut defined,
							remote_contexts.clone(),
							loader,
							base_url,
							protected,
							options,
							warnings,
						)
						.await
						.map_err(|e| e.at(binding.key_location().clone()))?
					}
				}
			}
		}

		Ok(result)
	}
	.boxed()
}