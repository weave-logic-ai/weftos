//! Criterion benchmarks for pipeline components.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

use clawft_core::agent::context::{CompressionConfig, compress_context, count_tokens};
use clawft_core::pipeline::mutation::{MutationStrategy, TrajectoryHint, mutate_prompt};
use clawft_core::pipeline::scorer::FitnessScorer;
use clawft_core::pipeline::traits::{ChatRequest, LlmMessage, QualityScorer};
use clawft_types::provider::{ContentBlock, LlmResponse, StopReason, Usage};

use std::collections::HashMap;

// ── Helpers ──────────────────────────────────────────────────────────────

fn make_msg(role: &str, content: &str) -> LlmMessage {
    LlmMessage {
        role: role.into(),
        content: content.into(),
        tool_call_id: None,
        tool_calls: None,
    }
}

fn generate_words(n: usize) -> String {
    // Deterministic word generation for reproducible benchmarks.
    (0..n)
        .map(|i| {
            // Cycle through varied word lengths for realistic token estimation.
            match i % 5 {
                0 => "the",
                1 => "implementation",
                2 => "of",
                3 => "vectorized",
                _ => "algorithms",
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn generate_messages(n: usize) -> Vec<LlmMessage> {
    let mut msgs = vec![make_msg("system", "You are a helpful coding assistant.")];
    for i in 0..n {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        let text = generate_words(20 + (i % 10) * 5);
        msgs.push(make_msg(role, &text));
    }
    msgs
}

fn make_request(content: &str) -> ChatRequest {
    ChatRequest {
        messages: vec![LlmMessage {
            role: "user".into(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }],
        tools: vec![],
        model: None,
        max_tokens: None,
        temperature: None,
        auth_context: None,
        complexity_boost: 0.0,
    }
}

fn make_response(text: &str, output_tokens: u32) -> LlmResponse {
    LlmResponse {
        id: "bench".into(),
        content: vec![ContentBlock::Text { text: text.into() }],
        stop_reason: StopReason::EndTurn,
        usage: Usage {
            input_tokens: 50,
            output_tokens,
            total_tokens: 0,
        },
        metadata: HashMap::new(),
    }
}

fn sample_trajectories() -> Vec<TrajectoryHint> {
    vec![
        TrajectoryHint {
            request_content: "Write a function to sort a list".into(),
            quality_score: 0.9,
            feedback: "Excellent quality.".into(),
        },
        TrajectoryHint {
            request_content: "Explain recursion with examples".into(),
            quality_score: 0.85,
            feedback: "Good explanation with clear examples.".into(),
        },
        TrajectoryHint {
            request_content: "Fix the bug in my code".into(),
            quality_score: 0.3,
            feedback: "Low relevance.".into(),
        },
        TrajectoryHint {
            request_content: "Summarize this article".into(),
            quality_score: 0.4,
            feedback: "Low coherence.".into(),
        },
    ]
}

// ── Benchmarks ───────────────────────────────────────────────────────────

fn bench_count_tokens(c: &mut Criterion) {
    let mut group = c.benchmark_group("count_tokens");

    for word_count in [10, 100, 1000, 10000] {
        let text = generate_words(word_count);
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}_words", word_count)),
            &text,
            |b, text| {
                b.iter(|| count_tokens(black_box(text)));
            },
        );
    }

    group.finish();
}

fn bench_compress_context(c: &mut Criterion) {
    let mut group = c.benchmark_group("compress_context");

    for msg_count in [10, 50, 100, 500] {
        let messages = generate_messages(msg_count);
        let config = CompressionConfig {
            max_context_tokens: 200, // Force compression
            recent_message_count: 5,
            compression_enabled: true,
        };

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}_messages", msg_count)),
            &(messages.clone(), config.clone()),
            |b, (msgs, cfg)| {
                b.iter(|| compress_context(black_box(msgs.clone()), black_box(cfg)));
            },
        );
    }

    group.finish();
}

fn bench_fitness_scorer(c: &mut Criterion) {
    let scorer = FitnessScorer::new();

    let request = make_request("Write a Rust function to parse JSON and validate the schema");
    let good_response = make_response(
        "Here is a function that parses JSON and validates it against a schema:\n\
         ```rust\nfn validate(json: &str) -> Result<(), Error> { todo!() }\n```\n\
         - Handles nested objects\n- Reports line numbers for errors",
        150,
    );

    c.bench_function("fitness_scorer_typical", |b| {
        b.iter(|| scorer.score(black_box(&request), black_box(&good_response)));
    });
}

fn bench_mutate_prompt(c: &mut Criterion) {
    let mut group = c.benchmark_group("mutate_prompt");
    let trajectories = sample_trajectories();
    let prompt = "Use the search tool to find relevant documentation.\n\
                  Ensure the output is well-structured.\n\
                  Always verify tool results before responding.\n\
                  Provide code examples when applicable.";

    for (name, strategy) in [
        ("rephrase", MutationStrategy::Rephrase),
        ("add_examples", MutationStrategy::AddExamples),
        ("remove_ineffective", MutationStrategy::RemoveIneffective),
        ("emphasize", MutationStrategy::Emphasize),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| mutate_prompt(black_box(prompt), black_box(&trajectories), strategy));
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_count_tokens,
    bench_compress_context,
    bench_fitness_scorer,
    bench_mutate_prompt,
);
criterion_main!(benches);
