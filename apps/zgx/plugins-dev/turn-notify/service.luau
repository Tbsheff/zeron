local zeron = require("./zeron")("turn-notify")

zeron.on("turn.done", function(turn: any)
	if turn.status == "interrupted" then
		return
	end
	local title = turn.title or "A chat"
	local word = if turn.status == "completed" then "finished" else "failed"
	zeron.notify({ title = `{title} {word}`, session = turn.session })
	zeron.emit("turn", { session = turn.session, title = title, word = word })
end)
