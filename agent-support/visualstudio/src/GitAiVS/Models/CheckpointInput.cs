using System.Collections.Generic;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace GitAiVS.Models
{
    /// <summary>
    /// Base class for agent-v1 checkpoint inputs sent to:
    ///   git-ai checkpoint agent-v1 --hook-input stdin
    /// 
    /// The JSON is tagged with "type" to match the Rust AgentV1Payload enum
    /// (deserialized via #[serde(tag = "type", rename_all = "snake_case")]).
    /// </summary>
    public abstract class AgentV1Input
    {
        [JsonPropertyName("type")]
        public abstract string Type { get; }

        [JsonPropertyName("repo_working_dir")]
        public string RepoWorkingDir { get; set; } = "";

        public abstract string ToJson();
    }

    /// <summary>
    /// Human (before_edit) checkpoint — captures file state before an AI edit begins.
    /// </summary>
    public sealed class HumanInput : AgentV1Input
    {
        [JsonPropertyName("type")]
        public override string Type => "human";

        [JsonPropertyName("will_edit_filepaths")]
        [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
        public List<string>? WillEditFilepaths { get; set; }

        [JsonPropertyName("dirty_files")]
        [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
        public Dictionary<string, string>? DirtyFiles { get; set; }

        public override string ToJson() => JsonSerializer.Serialize(this, JsonOptions.Default);
    }

    /// <summary>
    /// AI agent (after_edit) checkpoint — records changes made by an AI agent.
    /// </summary>
    public sealed class AiAgentInput : AgentV1Input
    {
        [JsonPropertyName("type")]
        public override string Type => "ai_agent";

        [JsonPropertyName("edited_filepaths")]
        [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
        public List<string>? EditedFilepaths { get; set; }

        [JsonPropertyName("agent_name")]
        public string AgentName { get; set; } = "";

        [JsonPropertyName("model")]
        public string Model { get; set; } = "unknown";

        [JsonPropertyName("conversation_id")]
        public string ConversationId { get; set; } = "";

        [JsonPropertyName("dirty_files")]
        [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
        public Dictionary<string, string>? DirtyFiles { get; set; }

        public override string ToJson() => JsonSerializer.Serialize(this, JsonOptions.Default);
    }

    /// <summary>
    /// Known-human checkpoint input sent to:
    ///   git-ai checkpoint known_human --hook-input stdin
    /// </summary>
    public sealed class KnownHumanInput
    {
        [JsonPropertyName("editor")]
        public string Editor { get; set; } = "visualstudio";

        [JsonPropertyName("editor_version")]
        public string EditorVersion { get; set; } = "";

        [JsonPropertyName("extension_version")]
        public string ExtensionVersion { get; set; } = "";

        [JsonPropertyName("cwd")]
        [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
        public string? Cwd { get; set; }

        [JsonPropertyName("edited_filepaths")]
        public List<string> EditedFilepaths { get; set; } = new();

        [JsonPropertyName("dirty_files")]
        public Dictionary<string, string> DirtyFiles { get; set; } = new();

        public string ToJson() => JsonSerializer.Serialize(this, JsonOptions.Default);
    }

    internal static class JsonOptions
    {
        public static readonly JsonSerializerOptions Default = new()
        {
            DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
            PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
            WriteIndented = false,
        };
    }
}
