local zeron = require("./zeron")("turn-notify")

zeron.service():on("turn", function(turn: any)
	zeron.toast(`{turn.title} {turn.word}`)
end)
