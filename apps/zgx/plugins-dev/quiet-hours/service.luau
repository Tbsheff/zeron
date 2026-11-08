local zeron = require("./zeron")("quiet-hours")

local HOUR = 3600

local function quiet(hour: number, from: number, to: number): boolean
	if from == to then
		return false
	elseif from < to then
		return hour >= from and hour < to
	end
	return hour >= from or hour < to
end

local function next_hour(now: number, hour: number): number
	local t = os.date("*t", now)
	t.hour, t.min, t.sec = hour, 0, 0
	local at = os.time(t)
	return if at > now then at else at + 24 * HOUR
end

zeron.hook("message.dispatch", function(ctx: any)
	if ctx.source == "person" then
		return nil
	end
	local from, to = zeron.setting("start"), zeron.setting("end")
	local now = os.time()
	if not quiet(os.date("*t", now).hour, from, to) then
		return nil
	end
	return { action = "wait", reason = `quiet hours until {to}:00`, ["until"] = next_hour(now, to) }
end)
