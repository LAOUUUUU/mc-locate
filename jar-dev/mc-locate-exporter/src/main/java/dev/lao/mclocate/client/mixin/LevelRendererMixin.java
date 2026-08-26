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

    @Inject(method = "renderLevel", at = @At("TAIL"), require = 0)
    private void mclocate$outline(CallbackInfo ci) {
        if (!Outlines.enabled) {
            return;
        }
        java.util.List<int[]> boxes = Outlines.snapshot();
        if (boxes.isEmpty()) {
            return;
        }
        net.minecraft.client.Minecraft mc = net.minecraft.client.Minecraft.getInstance();
        net.minecraft.client.multiplayer.ClientLevel lvl = mc.level;
        if (lvl == null) {
            return;
        }
        net.minecraft.client.Camera cam = mc.gameRenderer.getMainCamera();
        net.minecraft.world.phys.Vec3 camPos = cam.position();

        com.mojang.blaze3d.vertex.PoseStack pose = new com.mojang.blaze3d.vertex.PoseStack();
        pose.mulPose(cam.rotation().conjugate(new org.joml.Quaternionf()));

        net.minecraft.client.renderer.MultiBufferSource.BufferSource buffers =
                this.renderBuffers.bufferSource();
        // Draw each box twice: lines() gives the crisp edges you can see, and
        // secondaryBlockOutline() is vanilla's "show the outline through blocks"
        // type — together the whole box is visible even behind terrain. Depth
        // testing is baked into each render type in the new pipeline, so there is
        // no global toggle; two passes is how vanilla does it.
        com.mojang.blaze3d.vertex.VertexConsumer lines =
                buffers.getBuffer(net.minecraft.client.renderer.rendertype.RenderTypes.lines());
        com.mojang.blaze3d.vertex.VertexConsumer through =
                buffers.getBuffer(net.minecraft.client.renderer.rendertype.RenderTypes.secondaryBlockOutline());

        for (int[] b : boxes) {
            // Structure X/Z is exact but the reported bounding-box Y is not, so
            // anchor the box to the ground at its centre. Only do this for boxes
            // near the player: a far box sits in an unloaded chunk whose height
            // map is garbage (that was the "lines into the sky"), and skipping
            // them is faster too.
            int cx = (b[0] + b[3]) / 2;
            int cz = (b[2] + b[5]) / 2;
            if (Math.abs(cx - camPos.x) > 128 || Math.abs(cz - camPos.z) > 128) {
                continue;
            }
            // OCEAN_FLOOR, not WORLD_SURFACE, so an ocean ruin sits on the
            // seabed rather than floating up at the water surface.
            int surface = lvl.getHeight(
                    net.minecraft.world.level.levelgen.Heightmap.Types.OCEAN_FLOOR, cx, cz);
            int height = Math.max(4, Math.min(32, b[4] - b[1]));
            net.minecraft.world.phys.shapes.VoxelShape shape =
                    net.minecraft.world.phys.shapes.Shapes.create(new net.minecraft.world.phys.AABB(
                            b[0], surface - 1, b[2], b[3] + 1, surface - 1 + height, b[5] + 1));
            int color = b.length > 6 ? b[6] : 0xFF33FF66;
            int dim = (color & 0x00FFFFFF) | 0x80000000;
            net.minecraft.client.renderer.ShapeRenderer.renderShape(
                    pose, through, shape, -camPos.x, -camPos.y, -camPos.z, dim, 1.0F);
            net.minecraft.client.renderer.ShapeRenderer.renderShape(
                    pose, lines, shape, -camPos.x, -camPos.y, -camPos.z, color, 1.0F);
        }
        buffers.endBatch(net.minecraft.client.renderer.rendertype.RenderTypes.secondaryBlockOutline());
        buffers.endBatch(net.minecraft.client.renderer.rendertype.RenderTypes.lines());
    }
    *///?}
}
