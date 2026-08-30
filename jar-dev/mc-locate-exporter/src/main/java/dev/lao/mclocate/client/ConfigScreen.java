package dev.lao.mclocate.client;

import java.util.function.BooleanSupplier;
import java.util.function.Consumer;

import net.minecraft.client.gui.components.Button;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.network.chat.Component;

/**
 * A small settings screen for the exporter.
 *
 * <p>Built entirely from button widgets with no custom {@code render} override,
 * on purpose: 26.x's render rewrite changed {@code Screen.render} and
 * {@code GuiGraphics}, but {@code init}, {@code addRenderableWidget},
 * {@code Button.builder} and {@code setMessage} are stable across every
 * supported version. Letting the base class draw the widgets keeps one screen
 * working from 1.21 through 26.2 without touching the version-divergent drawing
 * APIs.
 */
public class ConfigScreen extends Screen {
    private final Config config;

    public ConfigScreen(Config config) {
        super(Component.literal("mc-locate exporter"));
        this.config = config;
    }

    @Override
    protected void init() {
        int w = 240;
        int h = 20;
        int x = this.width / 2 - w / 2;
        int y = this.height / 5;
        int gap = 24;

        // Master switch first — off makes the whole mod inert.
        addRenderableWidget(toggle(x, y, w, h, "mc-locate (master)",
                () -> config.enabled, v -> config.enabled = v));
        y += gap;
        addRenderableWidget(toggle(x, y, w, h, "Passive bedrock",
                () -> config.autoBedrock, v -> config.autoBedrock = v));
        y += gap;
        addRenderableWidget(toggle(x, y, w, h, "Auto End pillars",
                () -> config.autoPillars, v -> config.autoPillars = v));
        y += gap;
        addRenderableWidget(toggle(x, y, w, h, "Auto eye throws",
                () -> config.autoEyes, v -> config.autoEyes = v));
        y += gap;
        addRenderableWidget(toggle(x, y, w, h, "Announce structures",
                () -> config.announceStructures, v -> config.announceStructures = v));
        y += gap;
        addRenderableWidget(toggle(x, y, w, h, "Structure wireframe",
                () -> config.outline, v -> config.outline = v));
        y += gap;
        addRenderableWidget(toggle(x, y, w, h, "Detect structures (servers)",
                () -> config.detectStructures, v -> config.detectStructures = v));
        y += gap;
        addRenderableWidget(toggle(x, y, w, h, "Announce collection",
                () -> config.announce, v -> config.announce = v));
        y += gap;
        addRenderableWidget(toggle(x, y, w, h, "Action-bar HUD",
                () -> config.hud, v -> config.hud = v));
        y += gap + 8;
        addRenderableWidget(Button.builder(Component.literal("Done"), b -> onClose())
                .bounds(x, y, w, h).build());
    }

    private Button toggle(int x, int y, int w, int h, String label, BooleanSupplier get,
            Consumer<Boolean> set) {
        return Button.builder(labelFor(label, get.getAsBoolean()), btn -> {
            boolean next = !get.getAsBoolean();
            set.accept(next);
            config.save();
            btn.setMessage(labelFor(label, next));
        }).bounds(x, y, w, h).build();
    }

    private static Component labelFor(String label, boolean on) {
        return Component.literal(label + ": " + (on ? "§aON" : "§cOFF"));
    }

    @Override
    public void onClose() {
        config.save();
        super.onClose();
    }
}
