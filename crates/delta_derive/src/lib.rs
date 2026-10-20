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

fn parse_type_name(input: &DeriveInput) -> syn::Result<Option<LitStr>> {
    let mut name = None;
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("reflect")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                name = Some(meta.value()?.parse::<LitStr>()?);
                Ok(())
            } else {
                Err(meta.error("unknown reflect attribute on a type"))
            }
        })?;
    }
    Ok(name)
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
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new(
            input.span(),
            "Reflect can only be derived for structs",
        ));
    };
    let ident = &input.ident;
    let name = match parse_type_name(input)? {
        Some(lit) => quote! { #lit },
        None => quote! { concat!(module_path!(), "::", stringify!(#ident)) },
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
                registry.register::<Self>(
                    #name,
                    ::core::mem::size_of::<Self>(),
                    ::core::mem::align_of::<Self>(),
                    fields,
                )
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
