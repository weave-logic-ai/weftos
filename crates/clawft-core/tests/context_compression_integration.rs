//! Integration tests for context compression.
//!
//! Verifies that the context compression pipeline correctly handles
//! large conversations, stays within token budgets, and produces
//! round-trippable compressed contexts suitable for the LLM pipeline.

use std::sync::Arc;

use clawft_core::agent::context::{
    CompressedContext, CompressionConfig, ContextBuilder, compress_context, count_tokens,
};
use clawft_core::bootstrap::AppContext;
use clawft_core::pipeline::traits::LlmMessage;
use clawft_platform::NativePlatform;
use clawft_types::config::Config;
use clawft_types::session::Session;

fn make_msg(role: &str, content: &str) -> LlmMessage {
    LlmMessage {
        role: role.into(),
        content: content.into(),
        tool_call_id: None,
        tool_calls: None,
    }
}

// ---------------------------------------------------------------------------
// Test 1: 50+ messages triggers compression (messages get summarized)
// ---------------------------------------------------------------------------

#[test]
fn large_conversation_triggers_compression() {
    let config = CompressionConfig {
        max_context_tokens: 200, // tight budget forces compression
        recent_message_count: 10,
        compression_enabled: true,
    };

    // Build a context with a system prompt and 55 user+assistant pairs (110 messages).
    let mut messages = vec![make_msg("system", "You are a helpful assistant.")];
    for i in 0..55 {
        messages.push(make_msg(
            "user",
            &format!(
                "This is user message number {} which contains enough words to inflate tokens.",
                i
            ),
        ));
        messages.push(make_msg(
            "assistant",
            &format!(
                "This is assistant reply number {} with additional detail to increase token count.",
                i
            ),
        ));
    }

    let original_count = messages.len();
    let result = compress_context(messages, &config);

    // Compression must have kicked in: older messages get summarized.
    assert!(
        result.metadata.messages_summarized > 0,
        "expected summarization of older messages, but got 0 summarized"
    );

    // The output should have fewer messages than the input (system + summary + 10 recent).
    assert!(
        result.messages.len() < original_count,
        "expected fewer messages after compression, got {} (was {})",
        result.messages.len(),
        original_count
    );

    // The last 10 non-system messages should be the most recent conversation.
    let non_system: Vec<&LlmMessage> = result
        .messages
        .iter()
        .filter(|m| m.role != "system")
        .collect();
    assert_eq!(
        non_system.len(),
        10,
        "expected 10 recent messages, got {}",
        non_system.len()
    );

    // Verify the very last message is the final assistant reply.
    let last = result.messages.last().unwrap();
    assert_eq!(last.role, "assistant");
    assert!(last.content.contains("reply number 54"));
}

// ---------------------------------------------------------------------------
// Test 2: Compressed context has fewer messages than original
// ---------------------------------------------------------------------------

#[test]
fn compressed_context_reduces_message_count() {
    let config = CompressionConfig {
        max_context_tokens: 300,
        recent_message_count: 5,
        compression_enabled: true,
    };

    // Build a large message list that far exceeds the budget.
    let mut messages = vec![make_msg("system", "You are a helpful assistant.")];
    for i in 0..60 {
        messages.push(make_msg(
            "user",
            &format!(
                "Question number {} with enough detail to push the total way over budget.",
                i
            ),
        ));
        messages.push(make_msg(
            "assistant",
            &format!(
                "Answer number {} providing a thorough response to the question above.",
                i
            ),
        ));
    }

    let original_tokens: usize = messages.iter().map(|m| count_tokens(&m.content)).sum();
    assert!(
        original_tokens > config.max_context_tokens,
        "precondition: original tokens ({}) should exceed budget ({})",
        original_tokens,
        config.max_context_tokens
    );

    let original_count = messages.len();
    let result = compress_context(messages, &config);

    // Should have dramatically fewer messages (system + summary + 5 recent
    // vs 121 original).
    assert!(
        result.messages.len() < original_count,
        "compressed message count ({}) should be less than original ({})",
        result.messages.len(),
        original_count
    );

    // System prompt must survive compression.
    assert_eq!(result.messages[0].role, "system");
    assert!(result.messages[0].content.contains("helpful assistant"));

    // Recent messages (last 5 conversation messages) should be verbatim.
    let non_system: Vec<&LlmMessage> = result
        .messages
        .iter()
        .filter(|m| m.role != "system")
        .collect();
    assert_eq!(non_system.len(), 5);

    // The summarized count should be 115 (120 conversation - 5 recent).
    assert_eq!(result.metadata.messages_summarized, 115);
}

// ---------------------------------------------------------------------------
// Test 3: Round-trip -- build via AppContext, compress, use in pipeline
// ---------------------------------------------------------------------------

#[tokio::test]
async fn roundtrip_build_compress_pipeline() {
    let config = Config::default();
    let platform = Arc::new(NativePlatform::new());
    let app_ctx = AppContext::new(config, platform).await.unwrap();

    // Build a ContextBuilder from AppContext's public accessors.
    let ctx_builder = ContextBuilder::new(
        app_ctx.config().agents.clone(),
        app_ctx.memory().clone(),
        app_ctx.skills().clone(),
        app_ctx.platform().clone(),
    )
    .with_compression(CompressionConfig {
        max_context_tokens: 150,
        recent_message_count: 6,
        compression_enabled: true,
    });

    let mut session = Session::new("compress:roundtrip");
    for i in 0..50 {
        session.add_message(
            "user",
            &format!("User turn {} discussing an important topic at length.", i),
            None,
        );
        session.add_message(
            "assistant",
            &format!("Assistant turn {} with a detailed and helpful response.", i),
            None,
        );
    }

    let compressed: CompressedContext = ctx_builder.build_messages_compressed(&session, &[]).await;

    // The result should be a valid message list usable by the pipeline.
    assert!(
        !compressed.messages.is_empty(),
        "compressed messages should not be empty"
    );

    // First message must be system role (system prompt).
    assert_eq!(compressed.messages[0].role, "system");

    // There must be a summary message when compression is active.
    let has_summary = compressed
        .messages
        .iter()
        .any(|m| m.content.contains("Conversation Summary"));
    assert!(
        has_summary,
        "expected a conversation summary message after compression"
    );

    // All messages should have valid roles.
    for msg in &compressed.messages {
        assert!(
            ["system", "user", "assistant"].contains(&msg.role.as_str()),
            "unexpected role: {}",
            msg.role
        );
    }

    // The recent conversation messages should be the last 6 non-system messages.
    let conversation_msgs: Vec<&LlmMessage> = compressed
        .messages
        .iter()
        .filter(|m| m.role != "system")
        .collect();
    assert_eq!(
        conversation_msgs.len(),
        6,
        "expected 6 recent messages, got {}",
        conversation_msgs.len()
    );

    // Verify ordering: the last conversation message should be the most recent.
    let last_conv = conversation_msgs.last().unwrap();
    assert!(
        last_conv.content.contains("turn 49"),
        "last conversation message should be turn 49, got: {}",
        last_conv.content
    );

    // Metadata: messages were summarized.
    assert!(compressed.metadata.messages_summarized > 0);

    // Verify the compressed token count matches the actual content.
    let actual_tokens: usize = compressed
        .messages
        .iter()
        .map(|m| count_tokens(&m.content))
        .sum();
    assert_eq!(
        compressed.metadata.compressed_tokens, actual_tokens,
        "metadata compressed_tokens should match actual token count"
    );
}

// ---------------------------------------------------------------------------
// Test 4: No compression when context fits within budget
// ---------------------------------------------------------------------------

#[test]
fn no_compression_when_within_budget() {
    let config = CompressionConfig {
        max_context_tokens: 500_000,
        recent_message_count: 10,
        compression_enabled: true,
    };

    let mut messages = vec![make_msg("system", "Short prompt.")];
    for i in 0..20 {
        messages.push(make_msg("user", &format!("Short msg {i}")));
        messages.push(make_msg("assistant", &format!("Reply {i}")));
    }

    let result = compress_context(messages, &config);

    assert_eq!(
        result.metadata.compression_ratio, 1.0,
        "should not compress when within budget"
    );
    assert_eq!(result.metadata.messages_summarized, 0);
    assert_eq!(
        result.metadata.original_tokens,
        result.metadata.compressed_tokens
    );
}

// ---------------------------------------------------------------------------
// Test 5: Compression preserves all system messages
// ---------------------------------------------------------------------------

#[test]
fn compression_preserves_all_system_messages() {
    let mut messages = vec![
        make_msg("system", "System prompt: you are helpful."),
        make_msg("system", "# Skill: research\n\nYou are a researcher."),
        make_msg("system", "# Relevant Memory:\n\nRust is fast."),
    ];

    for i in 0..50 {
        messages.push(make_msg(
            "user",
            &format!("User message {} with lots of words to blow the budget.", i),
        ));
        messages.push(make_msg(
            "assistant",
            &format!("Response {} also long enough to exceed limits.", i),
        ));
    }

    let config = CompressionConfig {
        max_context_tokens: 100,
        recent_message_count: 4,
        compression_enabled: true,
    };

    let result = compress_context(messages, &config);

    // All 3 original system messages should be preserved (excluding the summary).
    let system_msgs: Vec<&LlmMessage> = result
        .messages
        .iter()
        .filter(|m| m.role == "system" && !m.content.contains("Conversation Summary"))
        .collect();
    assert!(
        system_msgs.len() >= 3,
        "all original system messages should be preserved, got {}",
        system_msgs.len()
    );
    assert!(system_msgs[0].content.contains("you are helpful"));
    assert!(system_msgs[1].content.contains("# Skill: research"));
    assert!(system_msgs[2].content.contains("# Relevant Memory"));
}

// ---------------------------------------------------------------------------
// Test 6: Empty conversation produces no summary
// ---------------------------------------------------------------------------

#[test]
fn empty_conversation_produces_no_summary() {
    let messages = vec![make_msg("system", "Short system prompt.")];

    let config = CompressionConfig {
        max_context_tokens: 5,
        recent_message_count: 10,
        compression_enabled: true,
    };

    let result = compress_context(messages, &config);

    assert_eq!(result.metadata.messages_summarized, 0);
    let has_summary = result
        .messages
        .iter()
        .any(|m| m.content.contains("Conversation Summary"));
    assert!(
        !has_summary,
        "should not have a summary for empty conversation"
    );
}
