//! `#[derive(Reflect)]` for delta components.
//!
//! ```ignore
//! #[derive(Reflect)]
//! #[reflect(name = "game::Position")]          // optional stable name
//! struct Position {
//!     x: f32,
//!     #[reflect(range(0.0, 100.0), tooltip = "Height in metres")]
//!     y: f32,
//!     #[reflect(hidden)]
//!     cache: u32,
//! }
//! ```
//!
//! The default name is `module_path!()::TypeName`, which stays stable when
//! types are reordered or other components are added, unlike a component id.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    Data,
    DeriveInput,
    Expr,
    Fields,
    Index,
    LitStr,
    Member,
    Token,
    parse_macro_input,
    spanned::Spanned,
};

#[proc_macro_derive(Reflect, attributes(reflect))]
pub fn derive_reflect(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[derive(Default)]
struct FieldAttrs {
    range: Option<(Expr, Expr)>,
    tooltip: Option<LitStr>,
    hidden: bool,
    read_only: bool,
}

struct TypeAttrs {
    name: Option<LitStr>,
    /// `#[reflect(script)]`: the engine generates a C# mirror.
    script: bool,
}

fn parse_type_attrs(input: &DeriveInput) -> syn::Result<TypeAttrs> {
    let mut out = TypeAttrs {
        name: None,
        script: false,
    };
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("reflect")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                out.name = Some(meta.value()?.parse::<LitStr>()?);
                Ok(())
            } else if meta.path.is_ident("script") {
                out.script = true;
                Ok(())
            } else {
                Err(meta.error("unknown reflect attribute on a type"))
            }
        })?;
    }
    Ok(out)
}

/// The words inside the item's `#[repr(..)]` attributes.
fn repr_words(input: &DeriveInput) -> Vec<String> {
    let mut words = Vec::new();
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("repr")) {
        let _ = attr.parse_nested_meta(|meta| {
            if let Some(id) = meta.path.get_ident() {
                words.push(id.to_string());
            }
            // `align(16)` and friends carry a value we do not need.
            if meta.input.peek(syn::token::Paren) {
                let content;
                syn::parenthesized!(content in meta.input);
                let _: TokenStream2 = content.parse()?;
            }
            Ok(())
        });
    }
    words
}

/// A fieldless enum: stored as its `#[repr(u8/u16/u32)]` integer.
fn expand_enum(
    input: &DeriveInput,
    data: &syn::DataEnum,
) -> syn::Result<TokenStream2> {
    let ident = &input.ident;
    let words = repr_words(input);
    let repr = if words.iter().any(|w| w == "u8") {
        quote! { U8 }
    } else if words.iter().any(|w| w == "u16") {
        quote! { U16 }
    } else if words.iter().any(|w| w == "u32") {
        quote! { U32 }
    } else {
        return Err(syn::Error::new(
            input.span(),
            "a reflected enum needs #[repr(u8)], #[repr(u16)] or #[repr(u32)]",
        ));
    };
    let mut variants = Vec::new();
    for v in &data.variants {
        if !matches!(v.fields, Fields::Unit) {
            return Err(syn::Error::new(
                v.span(),
                "only fieldless enums can be reflected",
            ));
        }
        let name = v.ident.to_string();
        let var = &v.ident;
        variants.push(quote! {
            (::std::boxed::Box::<str>::from(#name), Self::#var as u32)
        });
    }
    Ok(quote! {
        impl ::delta::reflect::ReflectField for #ident {
            fn field_type(
                _: &mut ::delta::reflect::TypeRegistry,
            ) -> ::delta::reflect::FieldType {
                ::delta::reflect::FieldType::Enum {
                    repr: ::delta::reflect::EnumRepr::#repr,
                    variants: ::std::vec![ #(#variants),* ].into(),
                }
            }
        }
    })
}

fn parse_field_attrs(field: &syn::Field) -> syn::Result<FieldAttrs> {
    let mut out = FieldAttrs::default();
    for attr in field.attrs.iter().filter(|a| a.path().is_ident("reflect")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("hidden") {
                out.hidden = true;
            } else if meta.path.is_ident("read_only") {
                out.read_only = true;
            } else if meta.path.is_ident("tooltip") {
                out.tooltip = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("range") {
                let content;
                syn::parenthesized!(content in meta.input);
                let lo: Expr = content.parse()?;
                content.parse::<Token![,]>()?;
                let hi: Expr = content.parse()?;
                out.range = Some((lo, hi));
            } else {
                return Err(meta.error("unknown reflect attribute on a field"));
            }
            Ok(())
        })?;
    }
    Ok(out)
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(
            input.generics.span(),
            "Reflect cannot be derived for generic types",
        ));
    }
    let data = match &input.data {
        Data::Struct(data) => data,
        Data::Enum(data) => return expand_enum(input, data),
        Data::Union(_) => {
            return Err(syn::Error::new(
                input.span(),
                "Reflect can only be derived for structs and fieldless enums",
            ));
        }
    };
    let ident = &input.ident;
    let attrs = parse_type_attrs(input)?;
    let name = match &attrs.name {
        Some(lit) => quote! { #lit },
        None => quote! { concat!(module_path!(), "::", stringify!(#ident)) },
    };
    if attrs.script && !repr_words(input).iter().any(|w| w == "C") {
        return Err(syn::Error::new(
            input.span(),
            "#[reflect(script)] needs #[repr(C)]: the C# mirror must \
             reproduce the layout exactly",
        ));
    }
    let mark_script = if attrs.script {
        quote! { registry.mark_script(id); }
    } else {
        quote! {}
    };

    let mut field_exprs = Vec::new();
    let fields: Vec<&syn::Field> = match &data.fields {
        Fields::Named(f) => f.named.iter().collect(),
        Fields::Unnamed(f) => f.unnamed.iter().collect(),
        Fields::Unit => Vec::new(),
    };
    for (i, field) in fields.iter().enumerate() {
        let (member, field_name) = match &field.ident {
            Some(id) => (
                Member::Named(id.clone()),
                id.to_string().trim_start_matches("r#").to_owned(),
            ),
            None => (Member::Unnamed(Index::from(i)), i.to_string()),
        };
        let attrs = parse_field_attrs(field)?;
        let range = match attrs.range {
            Some((lo, hi)) => quote! { Some((((#lo) as f64), ((#hi) as f64))) },
            None => quote! { None },
        };
        let tooltip = match attrs.tooltip {
            Some(t) => quote! { Some(#t.into()) },
            None => quote! { None },
        };
        let (hidden, read_only) = (attrs.hidden, attrs.read_only);
        let ty = &field.ty;
        field_exprs.push(quote! {
            ::delta::reflect::FieldReflect::new(
                #field_name,
                ::core::mem::offset_of!(Self, #member),
                <#ty as ::delta::reflect::ReflectField>::field_type(registry),
                ::delta::reflect::FieldAttrs {
                    range: #range,
                    tooltip: #tooltip,
                    hidden: #hidden,
                    read_only: #read_only,
                },
            )
        });
    }

    Ok(quote! {
        impl ::delta::reflect::Reflect for #ident {
            fn reflect_id(
                registry: &mut ::delta::reflect::TypeRegistry,
            ) -> ::delta::reflect::ReflectId {
                if let Some(id) = registry.id_of::<Self>() {
                    return id;
                }
                let fields = ::std::vec![ #(#field_exprs),* ];
                let id = registry.register::<Self>(
                    #name,
                    ::core::mem::size_of::<Self>(),
                    ::core::mem::align_of::<Self>(),
                    fields,
                );
                #mark_script
                id
            }
        }

        impl ::delta::reflect::ReflectField for #ident {
            fn field_type(
                registry: &mut ::delta::reflect::TypeRegistry,
            ) -> ::delta::reflect::FieldType {
                ::delta::reflect::FieldType::Struct(
                    <Self as ::delta::reflect::Reflect>::reflect_id(registry),
                )
            }
        }
    })
}
