package dev.lao.mclocate.client;

import net.fabricmc.api.ClientModInitializer;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Client entrypoint.
 *
 * <p>Client-only by design: everything this mod collects is read out of chunks
 * the client already has, so it needs no server side and works on any world you
 * can load.
 */
public class ExporterClient implements ClientModInitializer {
	public static final String MOD_ID = "mc-locate-exporter";
	public static final Logger LOGGER = LoggerFactory.getLogger(MOD_ID);

	@Override
	public void onInitializeClient() {
		LOGGER.info("mc-locate exporter ready");
	}
}
