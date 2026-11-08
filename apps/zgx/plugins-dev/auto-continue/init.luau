local zeron = require("./zeron")("auto-continue")

zeron.service():on("continued", function(e: any)
	zeron.toast(`Sent "continue" to {e.title or "a chat"}`)
end)
