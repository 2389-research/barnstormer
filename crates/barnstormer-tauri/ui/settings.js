const invoke = window.__TAURI__.core.invoke;

function setField(id, value) {
  const element = document.getElementById(id);
  element.value = value ?? "";
}

function getOptionalValue(id) {
  const value = document.getElementById(id).value.trim();
  return value === "" ? null : value;
}

async function loadSettings() {
  const settings = await invoke("load_settings");
  setField("default_provider", settings.default_provider);
  setField("default_model", settings.default_model);
  setField("anthropic_api_key", settings.anthropic_api_key);
  setField("anthropic_base_url", settings.anthropic_base_url);
  setField("openai_api_key", settings.openai_api_key);
  setField("openai_base_url", settings.openai_base_url);
  setField("gemini_api_key", settings.gemini_api_key);
  setField("gemini_base_url", settings.gemini_base_url);
}

document.getElementById("settings-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const status = document.getElementById("status");
  status.textContent = "Saving settings and launching Barnstormer...";

  try {
    await invoke("save_settings", {
      settings: {
        default_provider: document.getElementById("default_provider").value,
        default_model: getOptionalValue("default_model"),
        anthropic_api_key: getOptionalValue("anthropic_api_key"),
        anthropic_base_url: getOptionalValue("anthropic_base_url"),
        openai_api_key: getOptionalValue("openai_api_key"),
        openai_base_url: getOptionalValue("openai_base_url"),
        gemini_api_key: getOptionalValue("gemini_api_key"),
        gemini_base_url: getOptionalValue("gemini_base_url")
      }
    });
    status.textContent = "Launching Barnstormer...";
  } catch (error) {
    status.textContent = `Unable to save settings: ${error}`;
  }
});

loadSettings().catch((error) => {
  document.getElementById("status").textContent = `Unable to load settings: ${error}`;
});
