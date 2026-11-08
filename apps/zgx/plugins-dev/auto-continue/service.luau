local zeron = require("./zeron")("auto-continue")

local continued = zeron.store("continued", {})

zeron.on("turn.done", function(turn: any)
	if turn.status ~= "errored" then
		return
	end
	local sent = continued:get()
	if sent[turn.session] then
		return
	end
	sent[turn.session] = true
	continued:set(sent)
	zeron.chats.send(turn.session, "continue"):await()
	zeron.emit("continued", { session = turn.session, title = turn.title })
end)
