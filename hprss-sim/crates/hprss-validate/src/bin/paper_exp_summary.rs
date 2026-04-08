use hprss_validate::run_paper_experiment_summary;

fn main() {
    let report = run_paper_experiment_summary();

    println!("{}", report.to_table());
    println!("\n{}", report.to_csv());
    println!(
        "\n{}",
        report
            .to_json_pretty()
            .expect("paper experiment summary should serialize to JSON")
    );
}
