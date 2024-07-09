use proc_macro::TokenStream;
use quote::quote;
use syn::parse_macro_input;

#[proc_macro_attribute]
pub fn main(_attr: TokenStream, item: TokenStream) -> TokenStream {
    //let attr = parse_macro_input!(attr as syn::LitStr);
    let item = parse_macro_input!(item as syn::ItemFn);
    let rtype = &item.sig.output;
    let block = &item.block;

    let expanded = quote! {
        async fn __async_main() #rtype #block

        fn main() #rtype {
            env_logger::init();
            rsart::CONTEXT.set(rsart::Runtime::new().unwrap());
            rsart::block_on(__async_main())
        }
    };

    expanded.into()
}
