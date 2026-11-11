fn main() {
    let document = buizz_api::openapi()
        .to_pretty_json()
        .expect("the OpenAPI document serializes");
    println!("{document}");
}
