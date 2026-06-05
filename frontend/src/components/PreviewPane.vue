<script setup lang="ts">
import { highlightedParts } from '../domain/highlight'
import type { AroundResponse, ContextLine, PreviewMode, SearchHit } from '../domain/logs'

defineProps<{
  selectedHit: SearchHit
  expanded: boolean
  previewLines: ContextLine[]
  previewMode: PreviewMode
  expandRows: number
  aroundLoading: boolean
  around: AroundResponse | null
  highlightRegex: RegExp | null
}>()

const emit = defineEmits<{
  close: []
  toggleExpanded: []
  loadInitial: []
  updateExpandRows: [value: number]
  expandBefore: []
  expandAfter: []
  expandBoth: []
}>()

const EXPAND_OPTIONS = [20, 50, 100, 200]
</script>

<template>
  <div class="previewHeader">
    <div class="previewTitle">
      <span class="previewPath" :title="selectedHit.path">{{ selectedHit.path }}</span>
    </div>
    <div class="previewActions">
      <strong>第 {{ selectedHit.lineNo }} 行</strong>
      <label class="previewInlineSelect">
        每次
        <select :value="expandRows" @change="emit('updateExpandRows', Number(($event.target as HTMLSelectElement).value))">
          <option v-for="value in EXPAND_OPTIONS" :key="value" :value="value">
            {{ value }}
          </option>
        </select>
        行
      </label>
      <button
        type="button"
        class="contextButton"
        :disabled="aroundLoading || (previewMode === 'around' && !around?.hasBefore)"
        @click="previewMode === 'around' ? emit('expandBefore') : emit('loadInitial')"
      >
        向上
      </button>
      <button
        type="button"
        class="contextButton"
        :disabled="aroundLoading || (previewMode === 'around' && !around?.hasAfter)"
        @click="previewMode === 'around' ? emit('expandAfter') : emit('loadInitial')"
      >
        向下
      </button>
      <button type="button" class="contextButton primary" :disabled="aroundLoading" @click="previewMode === 'around' ? emit('expandBoth') : emit('loadInitial')">
        上下
      </button>
      <button class="previewActionButton" type="button" @click="emit('toggleExpanded')">
        {{ expanded ? '退出全屏' : '全屏' }}
      </button>
      <button class="previewClose" type="button" aria-label="关闭预览" title="关闭预览" @click="emit('close')">x</button>
    </div>
  </div>
  <div class="logViewport">
    <div
      v-for="line in previewLines"
      :key="line.lineNo"
      class="logLine"
      :class="{ matchLine: line.lineNo === selectedHit.lineNo }"
    >
      <span class="lineNumber">{{ line.lineNo }}</span>
      <code>
        <template v-for="(part, index) in highlightedParts(line.content, highlightRegex)" :key="index">
          <mark v-if="part.match">{{ part.text }}</mark>
          <template v-else>{{ part.text }}</template>
        </template>
      </code>
    </div>
  </div>
</template>
