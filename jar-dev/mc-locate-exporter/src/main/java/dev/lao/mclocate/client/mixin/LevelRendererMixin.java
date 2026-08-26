package dev.lao.mclocate.client.mixin;

import org.spongepowered.asm.mixin.Final;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import net.minecraft.client.renderer.LevelRenderer;

import dev.lao.mclocate.client.Outlines;

/**
 * Draws the structure wireframes by hooking the world renderer directly.
 *
 * <p>Fabric removed WorldRenderEvents in the 1.21.9 render rework and the
 * replacement is not out yet, so a mixin is the only way to draw in the world on
 * 1.21.11. The body is scoped to 1.21.11 (the only version whose render API is
 * verified); on every other version this is an empty, harmless mixin. The
 * injector uses require = 0 so a signature mismatch never crashes the game.
 */
@Mixin(LevelRenderer.class)
public class LevelRendererMixin {
    //? if >=1.21.11 <26 {
    /*@Shadow @Final private net.minecraft.client.renderer.RenderBuffers renderBuffers;

    // A see-through copy of vanilla's LINES type. Every built-in line/outline
    // type depth-tests (LEQUAL is the pipeline-builder default), so there is no
    // ready-made type that shows through terrain or water. This rebuilds LINES
    // field-for-field from its own live config and flips only the depth test off
    // — no guessed constants. Built once, lazily, and cached.
    private static net.minecraft.client.renderer.rendertype.RenderType mclocate$xray;
    private static boolean mclocate$xrayTried;
    // Set if drawing ever throws (e.g. a rendering mod rejects the pipeline), so
    // the feature disables itself with one log line instead of crashing frames.
    private static boolean mclocate$disabled;

    private static org.slf4j.Logger mclocate$log() {
        return org.slf4j.LoggerFactory.getLogger("mc-locate");
    }

    private static net.minecraft.client.renderer.rendertype.RenderType mclocate$xrayLines() {
        if (mclocate$xrayTried) {
            return mclocate$xray;
        }
        mclocate$xrayTried = true;
        try {
            com.mojang.blaze3d.pipeline.RenderPipeline src =
                    net.minecraft.client.renderer.RenderPipelines.LINES;
            com.mojang.blaze3d.pipeline.RenderPipeline.Builder pb =
                    com.mojang.blaze3d.pipeline.RenderPipeline.builder()
                            .withLocation("mc-locate:lines_xray")
                            .withVertexShader(src.getVertexShader())
                            .withFragmentShader(src.getFragmentShader())
                            .withVertexFormat(src.getVertexFormat(), src.getVertexFormatMode())
                            .withDepthTestFunction(
                                    com.mojang.blaze3d.platform.DepthTestFunction.NO_DEPTH_TEST)
                            .withDepthWrite(false)
                            .withColorWrite(src.isWriteColor(), src.isWriteAlpha())
                            .withCull(src.isCull())
                            .withPolygonMode(src.getPolygonMode())
                            .withColorLogic(src.getColorLogic())
                            .withDepthBias(src.getDepthBiasScaleFactor(), src.getDepthBiasConstant());
            if (src.getBlendFunction().isPresent()) {
                pb.withBlend(src.getBlendFunction().get());
            } else {
                pb.withoutBlend();
            }
            for (com.mojang.blaze3d.pipeline.RenderPipeline.UniformDescription u : src.getUniforms()) {
                if (u.type() != null && u.textureFormat() != null) {
                    pb.withUniform(u.name(), u.type(), u.textureFormat());
                } else if (u.type() != null) {
                    pb.withUniform(u.name(), u.type());
                }
            }
            for (String sampler : src.getSamplers()) {
                pb.withSampler(sampler);
            }
            net.minecraft.client.renderer.ShaderDefines defs = src.getShaderDefines();
            for (String flag : defs.flags()) {
                pb.withShaderDefine(flag);
            }
            for (java.util.Map.Entry<String, String> e : defs.values().entrySet()) {
                try {
                    pb.withShaderDefine(e.getKey(), Integer.parseInt(e.getValue()));
                } catch (NumberFormatException notInt) {
                    try {
                        pb.withShaderDefine(e.getKey(), Float.parseFloat(e.getValue()));
                    } catch (NumberFormatException notFloat) {
                        // A non-numeric define; the builder has no string form, so
                        // skip it. LINES defines none, so this never fires today.
                    }
                }
            }
            com.mojang.blaze3d.pipeline.RenderPipeline pipeline = pb.build();
            net.minecraft.client.renderer.rendertype.RenderSetup setup =
                    net.minecraft.client.renderer.rendertype.RenderSetup.builder(pipeline)
                            .bufferSize(1536)
                            .createRenderSetup();
            mclocate$xray = net.minecraft.client.renderer.rendertype.RenderType.create(
                    "mc-locate:lines_xray", setup);
        } catch (Throwable t) {
            mclocate$log().warn(
                    "mc-locate: could not build the see-through outline; using plain lines", t);
            mclocate$xray = null;
        }
        return mclocate$xray;
    }

    @Inject(method = "renderLevel", at = @At("TAIL"), require = 0)
    private void mclocate$outline(CallbackInfo ci) {
        if (mclocate$disabled || !Outlines.enabled) {
            return;
        }
        java.util.List<int[]> boxes = Outlines.snapshot();
        if (boxes.isEmpty()) {
            return;
        }
        try {
            net.minecraft.client.Minecraft mc = net.minecraft.client.Minecraft.getInstance();
            if (mc.level == null) {
                return;
            }
            net.minecraft.client.Camera cam = mc.gameRenderer.getMainCamera();
            net.minecraft.world.phys.Vec3 camPos = cam.position();

            com.mojang.blaze3d.vertex.PoseStack pose = new com.mojang.blaze3d.vertex.PoseStack();
            pose.mulPose(cam.rotation().conjugate(new org.joml.Quaternionf()));

            net.minecraft.client.renderer.MultiBufferSource.BufferSource buffers =
                    this.renderBuffers.bufferSource();

            // Prefer the see-through type so the box is visible through terrain and
            // water; fall back to plain lines if it could not be built.
            net.minecraft.client.renderer.rendertype.RenderType type = mclocate$xrayLines();
            if (type == null) {
                type = net.minecraft.client.renderer.rendertype.RenderTypes.lines();
            }
            com.mojang.blaze3d.vertex.VertexConsumer lines = buffers.getBuffer(type);

            for (int[] b : boxes) {
                // Skip boxes far from the camera — purely a work bound; 64 boxes of
                // 12 edges is nothing, but there is no point drawing the far ones.
                int cx = (b[0] + b[3]) / 2;
                int cz = (b[2] + b[5]) / 2;
                if (Math.abs(cx - camPos.x) > 512 || Math.abs(cz - camPos.z) > 512) {
                    continue;
                }
                // The structure's real bounding box, straight from the integrated
                // server's StructureStart — exact X/Y/Z. (An earlier heightmap
                // "anchor" is what floated the box above the structure.) +1 on the
                // max corner so the wireframe wraps the block extent, not its min
                // corners.
                net.minecraft.world.phys.shapes.VoxelShape shape =
                        net.minecraft.world.phys.shapes.Shapes.create(new net.minecraft.world.phys.AABB(
                                b[0], b[1], b[2], b[3] + 1, b[4] + 1, b[5] + 1));
                int color = b.length > 6 ? b[6] : 0xFF33FF66;
                net.minecraft.client.renderer.ShapeRenderer.renderShape(
                        pose, lines, shape, -camPos.x, -camPos.y, -camPos.z, color, 1.0F);
            }
            buffers.endBatch(type);
        } catch (Throwable t) {
            mclocate$disabled = true;
            mclocate$log().warn("mc-locate: outline render failed; disabling it for this session", t);
        }
    }
    *///?}
}
