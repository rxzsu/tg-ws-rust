<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from 'vue'
import {
  ShaderMount,
  grainGradientFragmentShader,
  GrainGradientShapes,
  ShaderFitOptions,
  type GrainGradientShape,
} from '@paper-design/shaders'

const props = withDefaults(defineProps<{
  colors: string[]
  colorBack?: string
  softness?: number
  intensity?: number
  noise?: number
  shape?: GrainGradientShape
  speed?: number
  /** Virtual world size the shader is rendered against */
  worldWidth?: number
  worldHeight?: number
}>(), {
  colorBack: '#000000',
  softness: 0.5,
  intensity: 0.5,
  noise: 0.25,
  shape: 'corners',
  speed: 1,
  worldWidth: 1280,
  worldHeight: 720,
})

const host = ref<HTMLDivElement | null>(null)
let mount: ShaderMount | null = null

/** hex (#rgb/#rgba/#rrggbb/#rrggbbaa) -> [r, g, b, a] in 0..1 */
function toVec4(hex: string): [number, number, number, number] {
  let h = hex.replace(/^#/, '')
  if (h.length === 3 || h.length === 4) {
    h = h.split('').map(c => c + c).join('')
  }
  if (h.length === 6) h += 'ff'
  const n = parseInt(h, 16)
  return [
    ((n >>> 24) & 0xff) / 255,
    ((n >>> 16) & 0xff) / 255,
    ((n >>> 8) & 0xff) / 255,
    (n & 0xff) / 255,
  ]
}

function buildUniforms() {
  return {
    // sizing (object sizing — shapes 4-7 use object UV)
    u_fit: ShaderFitOptions.cover,
    u_scale: 1,
    u_rotation: 0,
    u_originX: 0.5,
    u_originY: 0.5,
    u_offsetX: 0,
    u_offsetY: 0,
    u_worldWidth: props.worldWidth,
    u_worldHeight: props.worldHeight,
    // grain gradient
    u_colorBack: toVec4(props.colorBack),
    u_colors: props.colors.map(toVec4),
    u_colorsCount: props.colors.length,
    u_softness: props.softness,
    u_intensity: props.intensity,
    u_noise: props.noise,
    u_shape: GrainGradientShapes[props.shape],
  }
}

onMounted(() => {
  if (!host.value) return
  mount = new ShaderMount(
    host.value,
    grainGradientFragmentShader,
    buildUniforms(),
    undefined,
    props.speed,
  )
})

onUnmounted(() => {
  mount?.dispose()
  mount = null
})

watch(
  () => [props.colors, props.colorBack, props.softness, props.intensity, props.noise, props.shape],
  () => mount?.setUniforms(buildUniforms()),
)

watch(() => props.speed, (s) => {
  mount?.setSpeed(s)
})
</script>

<template>
  <div ref="host" class="shader-host" aria-hidden="true" />
</template>

<style scoped>
.shader-host {
  position: absolute;
  inset: 0;
  overflow: hidden;
}

.shader-host :deep(canvas) {
  display: block;
  width: 100%;
  height: 100%;
}
</style>
