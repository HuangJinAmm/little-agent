<script setup lang="ts">
import { ref, onMounted, onUnmounted, nextTick } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { marked } from "marked";
import DOMPurify from "dompurify";
import { Send, Bot, User, Loader2 } from "lucide-vue-next";

interface Message {
  id: string;
  role: "user" | "assistant";
  content: string;
}

interface ChatEvent {
  event: "Start" | "Token" | "Status" | "End" | "Error";
  data?: string;
}

const messages = ref<Message[]>([]);
const input = ref("");
const isGenerating = ref(false);
const currentStatus = ref("Thinking...");
const chatContainer = ref<HTMLElement | null>(null);

let unlisten: UnlistenFn | null = null;
let currentAssistantMessageId: string | null = null;

const scrollToBottom = async () => {
  await nextTick();
  if (chatContainer.value) {
    chatContainer.value.scrollTop = chatContainer.value.scrollHeight;
  }
};

const renderMarkdown = (text: string) => {
  if (!text) return "";
  return DOMPurify.sanitize(marked(text) as string);
};

const parseMessage = (content: string) => {
  const thinkMatch = content.match(/<think>([\s\S]*?)(?:<\/think>|$)/);
  if (thinkMatch) {
    const thinkContent = thinkMatch[1];
    const textContent = content.replace(/<think>[\s\S]*?(?:<\/think>|$)/, '');
    return { think: thinkContent.trim(), text: textContent.trim() };
  }
  return { think: null, text: content };
};

const sendMessage = async () => {
  if (!input.value.trim() || isGenerating.value) return;

  const query = input.value.trim();
  input.value = "";

  messages.value.push({
    id: Date.now().toString(),
    role: "user",
    content: query,
  });
  scrollToBottom();

  isGenerating.value = true;
  currentAssistantMessageId = null;

  try {
    await invoke("send_message", { query });
  } catch (err) {
    console.error("Failed to send message:", err);
    isGenerating.value = false;
  }
};

onMounted(async () => {
  unlisten = await listen<ChatEvent>("chat-event", (e) => {
    const payload = e.payload;

    if (payload.event === "Start") {
      isGenerating.value = true;
      currentStatus.value = "Thinking...";
      currentAssistantMessageId = Date.now().toString();
      messages.value.push({
        id: currentAssistantMessageId,
        role: "assistant",
        content: "",
      });
      scrollToBottom();
    } else if (payload.event === "Status" && payload.data) {
      currentStatus.value = payload.data;
    } else if (payload.event === "Token" && payload.data) {
      if (!currentAssistantMessageId) {
        currentAssistantMessageId = Date.now().toString();
        messages.value.push({
          id: currentAssistantMessageId,
          role: "assistant",
          content: "",
        });
      }
      
      const msg = messages.value.find((m) => m.id === currentAssistantMessageId);
      if (msg) {
        msg.content += payload.data;
      }
      scrollToBottom();
    } else if (payload.event === "End") {
      isGenerating.value = false;
      currentStatus.value = "Thinking...";
      currentAssistantMessageId = null;
    } else if (payload.event === "Error") {
      isGenerating.value = false;
      currentStatus.value = "Thinking...";
      currentAssistantMessageId = null;
      messages.value.push({
        id: Date.now().toString(),
        role: "assistant",
        content: `**Error:** ${payload.data}`,
      });
      scrollToBottom();
    }
  });
});

onUnmounted(() => {
  if (unlisten) {
    unlisten();
  }
});
</script>

<template>
  <div class="flex flex-col h-screen w-full bg-neutral-950 text-neutral-100 font-sans">
    <!-- Header -->
    <header class="flex items-center px-6 py-4 border-b border-neutral-800 bg-neutral-900/50 backdrop-blur shrink-0">
      <div class="flex items-center gap-3">
        <div class="w-8 h-8 rounded-lg bg-emerald-500/20 text-emerald-400 flex items-center justify-center">
          <Bot :size="20" />
        </div>
        <div>
          <h1 class="font-semibold text-neutral-100">Little Agent</h1>
          <p class="text-xs text-neutral-400">Powered by Tauri v2 + Vue3</p>
        </div>
      </div>
    </header>

    <!-- Chat Area -->
    <main 
      ref="chatContainer"
      class="flex-1 overflow-y-auto p-6 space-y-6 scroll-smooth"
    >
      <div v-if="messages.length === 0" class="flex flex-col items-center justify-center h-full text-neutral-500 space-y-4">
        <Bot :size="48" class="text-neutral-700" />
        <p>How can I help you today?</p>
      </div>

      <div 
        v-for="msg in messages" 
        :key="msg.id"
        class="flex gap-4 max-w-4xl mx-auto w-full"
        :class="msg.role === 'user' ? 'flex-row-reverse' : ''"
      >
        <div 
          class="w-8 h-8 rounded-full flex items-center justify-center shrink-0 mt-1"
          :class="msg.role === 'user' ? 'bg-blue-600 text-white' : 'bg-emerald-500/20 text-emerald-400'"
        >
          <User v-if="msg.role === 'user'" :size="16" />
          <Bot v-else :size="16" />
        </div>
        
        <div 
          class="px-4 py-3 rounded-2xl max-w-[80%]"
          :class="msg.role === 'user' 
            ? 'bg-blue-600 text-white rounded-tr-sm' 
            : 'bg-neutral-800/80 text-neutral-200 rounded-tl-sm prose prose-invert max-w-none'"
        >
          <div v-if="msg.role === 'user'" class="whitespace-pre-wrap">{{ msg.content }}</div>
          <div v-else>
            <details v-if="parseMessage(msg.content).think" :open="isGenerating && msg.id === currentAssistantMessageId && !msg.content.includes('</think>')" class="mb-4 bg-neutral-900/50 rounded-lg border border-neutral-700/50 overflow-hidden group">
              <summary class="cursor-pointer px-4 py-2 text-xs font-medium text-neutral-400 hover:text-neutral-300 bg-neutral-800/50 select-none flex items-center gap-2">
                <Loader2 v-if="isGenerating && msg.id === currentAssistantMessageId && !msg.content.includes('</think>')" class="animate-spin text-emerald-500" :size="12" />
                <span class="group-open:hidden">Show reasoning process</span>
                <span class="hidden group-open:inline">Hide reasoning process</span>
              </summary>
              <div class="p-4 text-sm text-neutral-400 whitespace-pre-wrap font-mono">{{ parseMessage(msg.content).think }}</div>
            </details>
            <div v-html="renderMarkdown(parseMessage(msg.content).text)"></div>
          </div>
        </div>
      </div>
      
      <div v-if="isGenerating && (!messages.length || messages[messages.length-1].role === 'user')" class="flex gap-4 max-w-4xl mx-auto w-full">
        <div class="w-8 h-8 rounded-full bg-emerald-500/20 text-emerald-400 flex items-center justify-center shrink-0 mt-1">
          <Bot :size="16" />
        </div>
        <div class="px-4 py-4 rounded-2xl bg-neutral-800/80 text-neutral-400 rounded-tl-sm flex items-center gap-2">
          <Loader2 class="animate-spin text-emerald-500" :size="16" />
          <span class="text-sm font-medium">{{ currentStatus }}</span>
        </div>
      </div>
    </main>

    <!-- Input Area -->
    <footer class="p-4 bg-neutral-900 border-t border-neutral-800 shrink-0">
      <div class="max-w-4xl mx-auto relative flex items-end gap-2">
        <textarea
          v-model="input"
          @keydown.enter.exact.prevent="sendMessage"
          rows="1"
          placeholder="Send a message..."
          class="w-full bg-neutral-800 border border-neutral-700 rounded-xl px-4 py-3.5 pr-12 text-neutral-100 placeholder-neutral-500 focus:outline-none focus:ring-2 focus:ring-emerald-500/50 focus:border-emerald-500/50 resize-none max-h-32"
          style="min-height: 52px"
        ></textarea>
        <button
          @click="sendMessage"
          :disabled="!input.trim() || isGenerating"
          class="absolute right-2 bottom-2 p-2 rounded-lg bg-emerald-500 text-white disabled:opacity-50 disabled:cursor-not-allowed hover:bg-emerald-600 transition-colors"
        >
          <Send :size="18" />
        </button>
      </div>
      <div class="text-center mt-2 text-[10px] text-neutral-500">
        Press Enter to send, Shift+Enter for new line
      </div>
    </footer>
  </div>
</template>

<style>
/* Additional global styles for markdown */
.prose pre {
  background-color: #171717 !important;
  border: 1px solid #262626;
  border-radius: 0.5rem;
  padding: 1rem;
}
.prose code {
  color: #e5e5e5;
  background-color: #262626;
  padding: 0.2rem 0.4rem;
  border-radius: 0.25rem;
  font-size: 0.875em;
}
.prose pre code {
  background-color: transparent;
  padding: 0;
  border-radius: 0;
}
</style>
