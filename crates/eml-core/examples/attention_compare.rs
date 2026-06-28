//! Head-to-head: ToyEmlAttention (SafeTree) vs BaselineAttention (plain affine).
//! Same CD optimizer, same trial budget, same data — compares substrate.

#[cfg(feature = "experimental-attention")]
fn main() {
    use eml_core::{compare_eml_vs_baseline, EndToEndTrainConfig};

    let cfg = EndToEndTrainConfig {
        trials: 5000,
        step_init: 0.5,
        step_final: 0.01,
        convergence_mse: 1e-3,
        ..Default::default()
    };
    let rounds = 3;
    let c = compare_eml_vs_baseline(4, 2, 2, 3, cfg, rounds).expect("compare");

    println!("Head-to-head: ToyEmlAttention (SafeTree) vs BaselineAttention (affine)");
    println!(
        "shape: d_model={} d_k={} seq_len={} depth={}",
        c.shape.0, c.shape.1, c.shape.2, c.shape.3
    );
    println!("trials/round: {}   rounds: {}", c.trials, c.rounds);
    println!();
    println!("{:<32} {:>12} {:>12}", "metric", "EML", "baseline");
    println!("{:-<32} {:->12} {:->12}", "", "", "");
    println!(
        "{:<32} {:>12} {:>12}",
        "param_count", c.eml_param_count, c.baseline_param_count,
    );
    println!(
        "{:<32} {:>12.4} {:>12.4}",
        "baseline_mse (untrained)", c.eml_baseline_mse, c.baseline_baseline_mse,
    );
    println!(
        "{:<32} {:>12.4} {:>12.4}",
        "final_mse", c.eml_final_mse, c.baseline_final_mse,
    );
    println!(
        "{:<32} {:>11.1}% {:>11.1}%",
        "mse reduction",
        c.eml_mse_reduction * 100.0,
        c.baseline_mse_reduction * 100.0,
    );
    println!(
        "{:<32} {:>10} ns {:>10} ns",
        "inference p99", c.eml_inference_ns_p99, c.baseline_inference_ns_p99,
    );
    println!();
    println!("Headline:");
    let eml_better_mse = c.eml_final_mse < c.baseline_final_mse;
    let baseline_better_speed = c.baseline_inference_ns_p99 < c.eml_inference_ns_p99;
    let speed_ratio = c.eml_inference_ns_p99 as f64 / c.baseline_inference_ns_p99.max(1) as f64;
    if eml_better_mse {
        println!(
            "  - EML reaches lower final MSE ({:.4} vs {:.4})",
            c.eml_final_mse, c.baseline_final_mse
        );
    } else {
        println!(
            "  - Baseline reaches lower final MSE ({:.4} vs {:.4})",
            c.baseline_final_mse, c.eml_final_mse
        );
    }
    println!(
        "  - EML inference is {:.2}x {} than baseline ({} vs {} ns p99)",
        speed_ratio.max(1.0 / speed_ratio),
        if baseline_better_speed {
            "slower"
        } else {
            "faster"
        },
        c.eml_inference_ns_p99,
        c.baseline_inference_ns_p99,
    );
    let param_ratio = c.eml_param_count as f64 / c.baseline_param_count.max(1) as f64;
    println!(
        "  - EML uses {:.2}x {} params than baseline ({} vs {})",
        param_ratio.max(1.0 / param_ratio),
        if c.eml_param_count > c.baseline_param_count {
            "more"
        } else {
            "fewer"
        },
        c.eml_param_count,
        c.baseline_param_count,
    );
}

#[cfg(not(feature = "experimental-attention"))]
fn main() {
    eprintln!("enable the `experimental-attention` feature");
    std::process::exit(2);
}
