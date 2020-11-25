use substrate_test_runtime_client::runtime::Block;

sp_api::decl_runtime_apis! {
	pub trait Api {
		fn test();
	}
}

struct MockApi;

sp_api::mock_impl_runtime_apis! {
	impl Api<Block> for MockApi {
		#[advanced]
		fn test(&self) -> Result<sp_core::NativeOrEncoded<()>, (&'static str, codec::Error)> {
			Ok(().into())
		}
	}
}

fn main() {}
